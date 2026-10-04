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
use crate::items::{InboxView, SidebarTab};
use crate::ui::agents::model::AgentRunSummary;

/// Whether a box is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    Empty,
    Full,
}

/// Where an item's history stands on its Agents section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemRunsPage {
    /// Older runs are left to load.
    pub has_more: bool,
    /// A page is on its way.
    pub loading: bool,
}

/// A View action waits for its item to finish opening before focusing the
/// discussion.  The token prevents an older, slower open from stealing a
/// newer request's focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAgentNavigation {
    pub token: u64,
    pub navigation_id: u64,
    pub item_id: String,
    pub review_attempt_id: Option<String>,
    /// A run to open in the item's Agents section once it is there.
    pub run_id: Option<String>,
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

/// Something worth surfacing wherever the reader is, in the popup at the
/// window's bottom right. The monotonically increasing token keeps an old
/// dismissal timer from clearing a newer toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub kind: ToastKind,
    pub token: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastKind {
    /// An agent run reached a terminal state.
    AgentRun(String),
    /// A line of news: something that could not be done, or was. One
    /// with `open` goes somewhere when clicked.
    Message {
        text: String,
        tone: Tone,
        open: Option<ToastOpen>,
    },
    /// A language server that isn't there, or is out of date, with the
    /// button that installs it. It stays until it's closed or the install
    /// ends; other news shows over it meanwhile.
    Server(ServerOffer),
}

/// A toast's offer to install or update a language server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerOffer {
    pub kind: perch_lsp::ServerKind,
    /// What the toast says.
    pub text: String,
    /// What its button runs, when Perch can install the server.
    pub install: Option<perch_lsp::Install>,
    /// Its button's label: Install, or Update.
    pub action: &'static str,
    /// Under which a closed offer stays closed for the session.
    pub key: String,
}

impl ServerOffer {
    /// The offer for a server that isn't there. `text` is what the toast
    /// says, the missing server's own line unless a goto has more to say.
    pub fn missing(missing: &perch_lsp::Missing, text: Option<String>) -> Self {
        ServerOffer {
            kind: missing.kind,
            text: text.unwrap_or_else(|| missing.describe()),
            install: missing.install.as_deref().cloned(),
            action: "Install",
            key: missing.describe(),
        }
    }
}

/// Where a message toast goes when clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastOpen {
    /// A pull request or issue, opened as anything is.
    Subject {
        repo_full_name: String,
        kind: perch_github::ItemKind,
        number: u64,
    },
    /// Send Feedback's preview, dropped down from the bug icon.
    FeedbackPreview,
}

/// The colour of a message's dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Error,
    Warning,
    Success,
}

#[derive(Clone, Copy)]
pub struct AppState {
    /// Each face as a PNG already cut to its circle (`backend::avatar`),
    /// shared so a clone is a refcount bump.
    pub avatars: RwSignal<HashMap<String, Arc<Vec<u8>>>>,
    /// Who acted last on each pull request and issue a look has found,
    /// most recent first, by repository and number: the faces an item's
    /// or a search hit's row stacks. A notification keeps its own on its
    /// snapshot.
    pub participants: RwSignal<HashMap<(String, u64), Vec<perch_github::Participant>>>,
    /// Declared repository identities and display names, also offered by
    /// the sidebar's Repository filter when they have no cached items.
    pub repos: RwSignal<Vec<DeclaredRepo>>,
    /// The hosts a pull request's or an issue's URL is read on.
    pub github_hosts: RwSignal<Vec<String>>,
    /// Open…'s box.
    pub open_box: crate::ui::open_box::OpenBox,
    /// What Browse's GitHub views have found, by search, until Perch
    /// quits.
    pub browse: RwSignal<HashMap<String, BrowsePage>>,
    /// Rises with every poll that goes through, for the Browse view on
    /// screen to be asked for again.
    pub polls: RwSignal<u64>,
    /// Every item, active and archived.
    pub items: RwSignal<Vec<Item>>,
    /// Perch's visible notification list, in GitHub response order after local
    /// Done watermarks have been applied.
    pub notifications: RwSignal<Vec<Thread>>,
    /// The threads marked Done in Perch that GitHub still lists, newest
    /// first: Inbox's View › Done.
    pub done_notifications: RwSignal<Vec<Thread>>,
    /// The threads snoozed in Perch that GitHub still lists, soonest back
    /// first: Inbox's View › Snoozed.
    pub snoozed_notifications: RwSignal<Vec<Thread>>,
    /// A poll's items the drawer is holding back because they would move
    /// its rows, until the reader asks for them or does something that
    /// sends the list again.
    pub pending_items: RwSignal<Option<Vec<Item>>>,
    /// The same for the notification list.
    pub pending_notifications: RwSignal<Option<Vec<Thread>>>,
    /// Whether a poll's notifications have arrived since launch. The first
    /// replaces the cached list outright rather than waiting to be asked.
    notifications_polled: RwSignal<bool>,
    /// The item whose row is selected in the sidebar: the open one.
    pub open_item: RwSignal<Option<String>>,
    /// The agent section of config.toml: binary, defaults, and the models
    /// the picker offers.
    pub agent: RwSignal<AgentConfig>,
    /// How much of each agent CLI's plan limits is used, as last heard.
    pub usage: RwSignal<perch_agent::usage::Board>,
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
    /// What was typed into each branch's pull request form, by item, until
    /// Perch quits or the pull request is opened.
    pub pull_request_drafts: RwSignal<HashMap<String, crate::review::open_pull::PullRequestDraft>>,
    /// What was typed into each pull request's Close as issue form, by
    /// item, until Perch quits or the issue is opened.
    pub issue_drafts: RwSignal<HashMap<String, crate::review::close_as_issue::IssueDraft>>,
    /// Ordered persisted events for details the reader has opened. The first
    /// history page does not pull every transcript into memory.
    pub agent_run_events:
        RwSignal<HashMap<String, Arc<Vec<perch_github::comments::RunEventRecord>>>>,
    /// Per-detail pagination state. Event pages are retained only for runs
    /// the reader has opened, and fetched from newest toward oldest.
    pub agent_run_events_has_more: RwSignal<HashMap<String, bool>>,
    pub agent_run_events_loading: RwSignal<HashSet<String>>,
    /// Whether the reader asked for the tree in the drawer on the right.
    /// Whether it shows is `tree_shown`: only on Code.
    pub tree_open: RwSignal<bool>,
    /// How wide the drawer is. Dragged, so a signal rather than a constant;
    /// docked, it may be shown narrower than this to leave the main pane
    /// its minimum.
    pub drawer_width: RwSignal<f64>,
    pub drawer_resizing: RwSignal<bool>,
    /// The window's width, from its last layout. The drawer takes room
    /// from the main pane while there is enough, and lies over it when
    /// there is not.
    pub window_width: RwSignal<f64>,
    /// The window's height, from its last layout, for a list to know
    /// whether it fits under what it hangs from.
    pub window_height: RwSignal<f64>,
    /// The main pane's left and right edges in the window, from its last
    /// layout, for Cmd-P's palette to centre itself over.
    pub main_pane: RwSignal<(f64, f64)>,
    /// Guards the one shared elapsed-time ticker; rows never make timers of
    /// their own.
    pub agents_clock_armed: RwSignal<bool>,
    /// The control the drawer was opened from, or whatever had the
    /// keyboard when it was opened by a shortcut, to hand it back to when
    /// the drawer shuts.
    pub drawer_opener: RwSignal<Option<floem::ViewId>>,
    pub agent_history_loading: RwSignal<bool>,
    pub agent_history_has_more: RwSignal<bool>,
    /// A load that received no answer within the UI's bounded wait. A later
    /// authoritative page clears this again.
    pub agent_history_load_failed: RwSignal<bool>,
    pub agent_history_load_token: RwSignal<u64>,
    /// Each item's history for its Agents section, by item: whether older runs
    /// are left to load, and whether a page is on its way.
    pub item_runs: RwSignal<HashMap<String, ItemRunsPage>>,
    /// Review group rows are expanded by default; this records the explicit
    /// collapses so newly admitted reviewer children are immediately visible.
    pub collapsed_agent_review_groups: RwSignal<HashSet<String>>,
    pub pending_agent_navigation: RwSignal<Option<PendingAgentNavigation>>,
    pub agent_navigation_token: RwSignal<u64>,
    expected_navigation_id: RwSignal<Option<u64>>,
    open_expectation: RwSignal<OpenExpectation>,
    /// What Start work leaves in the new branch's composer, by the
    /// navigation that opens the branch. Taken when that navigation lands.
    pub start_work_draft: RwSignal<Option<(u64, String)>>,
    /// An archived discussion opened from run history is a read-only view.
    /// Keep it open across the normal Items refresh without restoring its
    /// checkout or changing its archived state.
    pub agent_archived_owner: RwSignal<Option<String>>,
    /// Toasts are queued so simultaneous ones do not erase one another
    /// before the reader can see them.
    pub toasts: RwSignal<VecDeque<Toast>>,
    pub toast_token: RwSignal<u64>,
    /// The language servers being installed, from any window.
    pub installing_servers: RwSignal<HashSet<perch_lsp::ServerKind>>,
    /// The server offers closed this session, by their keys.
    pub declined_offers: RwSignal<HashSet<String>>,
    /// Send Feedback's draft. One for the app: every window's state holds
    /// the same signal (`windows::window`).
    pub feedback: RwSignal<Option<crate::feedback::FeedbackDraft>>,
    /// The session's last errors and warnings, for a report. Shared as
    /// `feedback` is.
    pub recent_problems: RwSignal<VecDeque<crate::feedback::Problem>>,

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
    /// Items whose review was asked for and has not reached the UI yet,
    /// with the newest group number they had then. Capturing the source
    /// can take seconds on a large repository, and Review should not sit
    /// unchanged meanwhile.
    pub review_launching: RwSignal<HashMap<String, u64>>,

    /// The item the pointer is over, so a row can show its buttons and a
    /// bare keystroke knows what it means.
    pub hovered_item: RwSignal<Option<String>>,
    /// The sidebar's filter menu settings and search.
    pub item_filters: RwSignal<crate::items::ItemFilters>,
    /// The item whose title is a text box right now.
    pub renaming: RwSignal<Option<String>>,
    /// The item with a Delete confirmation open under it.
    pub confirm_delete: RwSignal<Option<String>>,
    /// An archive or delete the backend stopped before it lost something,
    /// open under its row until the reader answers.
    pub confirm_loss: RwSignal<Option<crate::backend::LossPrompt>>,
    /// What Start work would cut from, while its confirmation is up.
    pub start_work_confirm: RwSignal<Option<crate::review::start_work::Pending>>,
    /// The new-work view and what it holds between visits.
    pub new_work: crate::ui::new_work::NewWorkState,
    /// A repository Add repository just put in the config, for the
    /// new-work view to choose once something with the backend sees it.
    pub added_repo: RwSignal<Option<String>>,
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
    /// The message of the poll failure last popped up, so the same one on
    /// every poll pops up once. None once a poll goes through.
    pub last_poll_problem: RwSignal<Option<String>>,
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
            participants: RwSignal::new(HashMap::new()),
            repos: RwSignal::new(Vec::new()),
            github_hosts: RwSignal::new(vec!["github.com".to_string()]),
            open_box: crate::ui::open_box::OpenBox::new(),
            browse: RwSignal::new(HashMap::new()),
            polls: RwSignal::new(0),
            items: RwSignal::new(Vec::new()),
            notifications: RwSignal::new(Vec::new()),
            done_notifications: RwSignal::new(Vec::new()),
            snoozed_notifications: RwSignal::new(Vec::new()),
            pending_items: RwSignal::new(None),
            pending_notifications: RwSignal::new(None),
            notifications_polled: RwSignal::new(false),
            open_item: RwSignal::new(None),
            review: RwSignal::new(None),
            viewer: RwSignal::new(None),
            reload: RwSignal::new(Vec::new()),
            agent_runs: RwSignal::new(Arc::new(Vec::new())),
            pull_request_drafts: RwSignal::new(HashMap::new()),
            issue_drafts: RwSignal::new(HashMap::new()),
            agent_run_events: RwSignal::new(HashMap::new()),
            agent_run_events_has_more: RwSignal::new(HashMap::new()),
            agent_run_events_loading: RwSignal::new(HashSet::new()),
            tree_open: RwSignal::new(false),
            drawer_width: RwSignal::new(crate::theme::DRAWER_WIDTH),
            drawer_resizing: RwSignal::new(false),
            window_width: RwSignal::new(0.0),
            window_height: RwSignal::new(0.0),
            main_pane: RwSignal::new((0.0, 0.0)),
            agents_clock_armed: RwSignal::new(false),
            drawer_opener: RwSignal::new(None),
            agent_history_loading: RwSignal::new(false),
            agent_history_has_more: RwSignal::new(false),
            item_runs: RwSignal::new(HashMap::new()),
            agent_history_load_failed: RwSignal::new(false),
            agent_history_load_token: RwSignal::new(0),
            collapsed_agent_review_groups: RwSignal::new(HashSet::new()),
            pending_agent_navigation: RwSignal::new(None),
            agent_navigation_token: RwSignal::new(0),
            expected_navigation_id: RwSignal::new(None),
            open_expectation: RwSignal::new(OpenExpectation::Any),
            start_work_draft: RwSignal::new(None),
            agent_archived_owner: RwSignal::new(None),
            toasts: RwSignal::new(VecDeque::new()),
            toast_token: RwSignal::new(0),
            installing_servers: RwSignal::new(HashSet::new()),
            declined_offers: RwSignal::new(HashSet::new()),
            feedback: RwSignal::new(None),
            recent_problems: RwSignal::new(VecDeque::new()),
            review_capabilities: RwSignal::new(Arc::new(Vec::new())),
            review_preferences: RwSignal::new(Arc::new(Vec::new())),
            review_base_refs: RwSignal::new(HashMap::new()),
            review_groups: RwSignal::new(HashMap::new()),
            review_findings: RwSignal::new(HashMap::new()),
            review_launch_errors: RwSignal::new(HashMap::new()),
            review_dashboard_loading: RwSignal::new(HashSet::new()),
            review_launching: RwSignal::new(HashMap::new()),
            agent: RwSignal::new(AgentConfig::default()),
            usage: RwSignal::new(Default::default()),
            hovered_item: RwSignal::new(None),
            item_filters: RwSignal::new(crate::items::ItemFilters::default()),
            renaming: RwSignal::new(None),
            confirm_delete: RwSignal::new(None),
            confirm_loss: RwSignal::new(None),
            start_work_confirm: RwSignal::new(None),
            new_work: crate::ui::new_work::NewWorkState::new(),
            added_repo: RwSignal::new(None),
            search_box: RwSignal::new(None),
            focus_search: RwSignal::new(false),
            item_query: RwSignal::new(String::new()),
            pending_meta: RwSignal::new(None),
            status: RwSignal::new(Status::Refreshing),
            last_poll_problem: RwSignal::new(None),
            now: RwSignal::new(Timestamp::now()),
            sidebar_open: RwSignal::new(true),
            sidebar_width: RwSignal::new(crate::theme::SIDEBAR_WIDTH),
            sidebar_resizing: RwSignal::new(false),
            search_focused: RwSignal::new(false),
        }
    }

    /// Start a new window from what `data` has heard from the backend, the
    /// rows, the inbox and the agents, and with `settings`' way of showing
    /// them. The backend sends most of it once, at launch.
    pub fn seed_from(&self, data: &AppState, settings: &AppState) {
        fn copy<T: Clone + 'static>(to: RwSignal<T>, from: RwSignal<T>) {
            to.set(from.get_untracked());
        }
        copy(self.avatars, data.avatars);
        copy(self.repos, data.repos);
        copy(self.github_hosts, data.github_hosts);
        copy(self.browse, data.browse);
        copy(self.items, data.items);
        copy(self.notifications, data.notifications);
        copy(self.notifications_polled, data.notifications_polled);
        copy(self.agent, data.agent);
        copy(self.usage, data.usage);
        copy(self.viewer, data.viewer);
        copy(self.agent_runs, data.agent_runs);
        copy(self.agent_run_events, data.agent_run_events);
        copy(self.agent_run_events_has_more, data.agent_run_events_has_more);
        copy(self.agent_history_has_more, data.agent_history_has_more);
        copy(self.item_runs, data.item_runs);
        copy(self.review_capabilities, data.review_capabilities);
        copy(self.review_preferences, data.review_preferences);
        copy(self.review_base_refs, data.review_base_refs);
        copy(self.review_groups, data.review_groups);
        copy(self.review_findings, data.review_findings);
        copy(self.status, data.status);
        copy(self.now, data.now);
        copy(self.new_work.prefix, data.new_work.prefix);
        copy(self.item_filters, settings.item_filters);
        copy(self.tree_open, settings.tree_open);
        copy(self.drawer_width, settings.drawer_width);
        copy(self.sidebar_open, settings.sidebar_open);
        copy(self.sidebar_width, settings.sidebar_width);
        if let (Some(to), Some(data), Some(settings)) = (
            self.review.get_untracked(),
            data.review.get_untracked(),
            settings.review.get_untracked(),
        ) {
            copy(to.approvals, data.approvals);
            copy(to.grants, data.grants);
            copy(to.run_changes, data.run_changes);
            copy(to.layout, settings.layout);
        }
    }

    /// Replace the summaries loaded at startup or by History pagination.
    /// Active records win by id; repeated pages are harmless.
    pub fn merge_agent_runs(
        &self,
        runs: impl IntoIterator<Item = perch_github::RunSummary>,
        has_more: bool,
    ) {
        self.merge_run_summaries(runs);
        self.agent_history_has_more.set(has_more);
        self.agent_history_loading.set(false);
        self.agent_history_load_failed.set(false);
    }

    /// Ask for a page of an item's runs for its Agents section: the first, or
    /// the one after the oldest it has.
    pub fn load_item_runs(&self, backend: &crate::backend::Backend, item_id: &str, older: bool) {
        const PAGE: usize = 20;
        let before = older
            .then(|| {
                self.agent_runs.with_untracked(|runs| {
                    runs.iter()
                        .filter(|run| run.item_id.as_deref() == Some(item_id) && !run.status.is_active())
                        .filter_map(|run| run.ended_at.map(|ended_at| (ended_at, run.id.clone())))
                        .min()
                        .map(|(ended_at, id)| perch_github::RunHistoryCursor { ended_at, id })
                })
            })
            .flatten();
        self.item_runs.update(|all| {
            all.entry(item_id.to_string()).or_default().loading = true;
        });
        backend.load_item_runs(item_id, before, PAGE);
    }

    fn merge_item_runs(
        &self,
        item_id: String,
        runs: impl IntoIterator<Item = perch_github::RunSummary>,
        has_more: bool,
    ) {
        self.merge_run_summaries(runs);
        self.item_runs.update(|all| {
            all.insert(item_id, ItemRunsPage { has_more, loading: false });
        });
    }

    fn merge_run_summaries(&self, runs: impl IntoIterator<Item = perch_github::RunSummary>) {
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
    }

    pub fn begin_review_dashboard_load(&self, item_id: &str) {
        self.review_dashboard_loading
            .update(|loading| _ = loading.insert(item_id.to_string()));
        self.review_launch_errors
            .update(|errors| _ = errors.remove(item_id));
    }

    pub fn begin_review_launch(&self, item_id: &str) {
        let newest = self
            .review_groups_for(item_id)
            .iter()
            .map(|summary| summary.group.number)
            .max()
            .unwrap_or(0);
        self.review_launching
            .update(|launching| _ = launching.insert(item_id.to_string(), newest));
        self.review_launch_errors
            .update(|errors| _ = errors.remove(item_id));
    }

    pub fn is_review_launching(&self, item_id: &str) -> bool {
        self.review_launching
            .with(|launching| launching.contains_key(item_id))
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
        // An older group's news does not mean the new one has arrived.
        if self.review_launching.with_untracked(|launching| {
            launching
                .get(&item_id)
                .is_some_and(|newest| summary.group.number > *newest)
        }) {
            self.review_launching
                .update(|launching| _ = launching.remove(&item_id));
        }
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
        self.review_launching
            .update(|launching| _ = launching.remove(&item_id));
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
        self.show_toast(ToastKind::AgentRun(run_id));
    }

    /// Put `text` up in the popup. It replaces any message still up or
    /// waiting, so a burst of complaints ends with the latest; an empty
    /// one only takes the old ones down.
    pub fn notify(&self, text: impl Into<String>, tone: Tone) {
        self.notify_with(text, tone, None);
    }

    /// [`Self::notify`], going to `open` when clicked. An error or a
    /// warning is kept for a bug report too.
    pub fn notify_with(&self, text: impl Into<String>, tone: Tone, open: Option<ToastOpen>) {
        let text = text.into();
        self.drop_toasts(|kind| matches!(kind, ToastKind::Message { .. }));
        if text.is_empty() {
            return;
        }
        if tone != Tone::Success {
            self.remember_problem(&text, tone);
        }
        self.show_toast(ToastKind::Message { text, tone, open });
    }

    /// Keep an error or warning for Send Feedback. Every window hears a
    /// poll's complaint and they share the list, so the same line again
    /// within a few seconds is the same problem.
    fn remember_problem(&self, text: &str, tone: Tone) {
        let at = Timestamp::now();
        let repeated = self.recent_problems.with_untracked(|problems| {
            problems.back().is_some_and(|last| {
                last.text == text && (at - last.at).get_seconds().abs() < 3
            })
        });
        if repeated {
            return;
        }
        self.recent_problems.update(|problems| {
            crate::feedback::remember_problem(
                problems,
                crate::feedback::Problem {
                    at,
                    text: text.to_string(),
                    tone,
                },
            )
        });
    }

    /// Something that could not be done, in the popup.
    pub fn complain(&self, text: impl Into<String>) {
        self.notify(text, Tone::Error);
    }

    /// The message up or waiting in the popup, if any.
    #[cfg(test)]
    pub fn notice(&self) -> Option<String> {
        self.toasts.with(|toasts| {
            toasts.iter().find_map(|toast| match &toast.kind {
                ToastKind::Message { text, .. } => Some(text.clone()),
                ToastKind::AgentRun(_) | ToastKind::Server(_) => None,
            })
        })
    }

    fn show_toast(&self, kind: ToastKind) {
        let token = self.toast_token.get_untracked().wrapping_add(1);
        self.toast_token.set(token);
        let was_empty = self.toasts.with_untracked(VecDeque::is_empty);
        // News goes in front of an offer that is waiting to be answered,
        // which comes back once the news has gone.
        let over_offer = !matches!(kind, ToastKind::Server(_))
            && self.toasts.with_untracked(|toasts| {
                toasts
                    .front()
                    .is_some_and(|toast| matches!(toast.kind, ToastKind::Server(_)))
            });
        self.toasts.update(|toasts| {
            if over_offer {
                toasts.push_front(Toast { kind, token });
            } else {
                toasts.push_back(Toast { kind, token });
            }
        });
        if was_empty || over_offer {
            self.schedule_toast_dismissal(token);
        }
    }

    /// Offer to install a language server, unless the same offer is up
    /// already or, for one the reader didn't ask for, was closed this
    /// session. `asked` is a goto that couldn't be answered for want of
    /// the server, which offers it again whatever was closed.
    pub fn offer_server(&self, offer: ServerOffer, asked: bool) {
        if !asked && self.declined_offers.with_untracked(|d| d.contains(&offer.key)) {
            return;
        }
        let shown = self.toasts.with_untracked(|toasts| {
            toasts.iter().any(|toast| match &toast.kind {
                ToastKind::Server(up) => up.key == offer.key,
                _ => false,
            })
        });
        if asked {
            self.declined_offers.update(|d| _ = d.remove(&offer.key));
            if shown {
                // The goto's line over the offer already up.
                self.toasts.update(|toasts| {
                    for toast in toasts.iter_mut() {
                        if let ToastKind::Server(up) = &mut toast.kind {
                            if up.key == offer.key {
                                up.text = offer.text.clone();
                            }
                        }
                    }
                });
                return;
            }
        } else if shown {
            return;
        }
        self.show_toast(ToastKind::Server(offer));
    }

    /// The reader closed an offer: it stays closed for the session.
    pub fn decline_offer(&self, token: u64, key: &str) {
        self.declined_offers.update(|d| _ = d.insert(key.to_string()));
        self.dismiss_toast(token);
    }

    /// Take down the offers for `kind`.
    fn drop_offers(&self, kind: perch_lsp::ServerKind) {
        self.drop_toasts(|toast| matches!(toast, ToastKind::Server(offer) if offer.kind == kind));
    }

    pub fn current_toast(&self) -> Option<Toast> {
        self.toasts.with(|toasts| toasts.front().cloned())
    }

    pub fn dismiss_toast(&self, token: u64) {
        let mut next = None;
        self.toasts.update(|toasts| {
            let Some(index) = toasts.iter().position(|toast| toast.token == token) else {
                return;
            };
            let was_front = index == 0;
            toasts.remove(index);
            if was_front {
                next = toasts.front().map(|toast| toast.token);
            }
        });
        if let Some(next) = next {
            self.schedule_toast_dismissal(next);
        }
    }

    /// Take down every toast `drop` picks, and start the clock on the one
    /// that comes to the front for it.
    fn drop_toasts(&self, drop: impl Fn(&ToastKind) -> bool) {
        if !self
            .toasts
            .with_untracked(|toasts| toasts.iter().any(|toast| drop(&toast.kind)))
        {
            return;
        }
        let front = |toasts: &VecDeque<Toast>| toasts.front().map(|toast| toast.token);
        let before = self.toasts.with_untracked(front);
        self.toasts
            .update(|toasts| toasts.retain(|toast| !drop(&toast.kind)));
        let next = self.toasts.with_untracked(front);
        if let Some(next) = next.filter(|next| Some(*next) != before) {
            self.schedule_toast_dismissal(next);
        }
    }

    fn schedule_toast_dismissal(&self, token: u64) {
        let state = *self;
        // An offer waits for its answer.
        let offer = self.toasts.with_untracked(|toasts| {
            toasts
                .iter()
                .any(|toast| toast.token == token && matches!(toast.kind, ToastKind::Server(_)))
        });
        if offer {
            return;
        }
        floem::action::exec_after(Duration::from_secs(8), move |_| {
            if state
                .toasts
                .with_untracked(|toasts| toasts.front().is_some_and(|toast| toast.token == token))
            {
                state.dismiss_toast(token);
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

    /// Keep an open run's detail live without retaining every transcript
    /// in the app signal: a run's events are here once its detail has been
    /// asked for, by opening it in an Agents section. The same event already
    /// committed before AgentStep was published; its authoritative sequence
    /// comes from AgentRunUpdated.
    fn append_open_agent_event(&self, run_id: &str, event: &perch_agent::RunEvent) {
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
            perch_agent::RunEvent::Steered(text) => {
                perch_github::RunEvent::SteerLanded { text: text.clone() }
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
            perch_agent::RunEvent::Report { .. }
            | perch_agent::RunEvent::Approval { .. }
            | perch_agent::RunEvent::Finished { .. }
            | perch_agent::RunEvent::Failed { .. }
            | perch_agent::RunEvent::Stopped { .. } => return,
        };
        self.append_open_agent_record(run_id, event);
    }

    /// Add `event` to the open detail of run `run_id`, if it is open, at
    /// the sequence its summary has reached.
    fn append_open_agent_record(&self, run_id: &str, event: perch_github::RunEvent) {
        if !self
            .agent_run_events
            .with_untracked(|events| events.contains_key(run_id))
        {
            return;
        }
        let Some(sequence) = self.agent_run(run_id).map(|run| run.last_event_sequence) else {
            return;
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
        self.drop_toasts(|kind| matches!(kind, ToastKind::AgentRun(id) if removed.contains(id)));
    }

    /// Whether the open item has a tree to show: it is open, on Code, with
    /// files. An item on its way has none, so the tree waits for Code.
    /// Tracked.
    pub fn tree_available(&self) -> bool {
        self.review.get().is_some_and(|review| {
            review.is_open()
                && review.tab.get() == crate::review::model::ItemTab::Code
                && review.has_files()
        })
    }

    /// Whether the drawer shows the tree: asked for, and on Code. Asked
    /// for on Conversation, or on an item with no files, the wish is kept
    /// for when there is a tree to show. Tracked.
    pub fn tree_shown(&self) -> bool {
        self.tree_open.get() && self.tree_available()
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

    /// Open a run's item on Conversation, with the run open in its Agents
    /// row once the item's threads have landed.
    pub fn begin_agent_run_navigation(
        &self,
        item_id: String,
        run_id: String,
        navigation_id: u64,
    ) -> u64 {
        let token = self.begin_agent_navigation_to(item_id, None, navigation_id);
        self.pending_agent_navigation.update(|pending| {
            if let Some(pending) = pending {
                pending.run_id = Some(run_id);
            }
        });
        token
    }

    pub fn begin_agent_review_navigation(
        &self,
        review: crate::review::model::ReviewState,
        item_id: String,
        review_attempt_id: String,
        navigation_id: u64,
    ) -> u64 {
        review.begin_historical_review_rollback(self.expected_navigation_id.get_untracked());
        self.begin_agent_navigation_to(item_id, Some(review_attempt_id), navigation_id)
    }

    fn begin_agent_navigation_to(
        &self,
        item_id: String,
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
                review_attempt_id,
                run_id: None,
            }));
        token
    }

    pub(crate) fn accepts_navigation(&self, navigation_id: u64) -> bool {
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
                // A pull request already showing: the progress is its
                // files', in Code, and the conversation stays up.
                if review.files_pending.with_untracked(Option::is_some) {
                    review
                        .files_pending
                        .set(Some(Preparing::Working { message, percent }));
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
                scope,
                files,
                tree,
                stale_head,
                offline,
                shown_early,
            } => {
                if !self.accepts_open_item(navigation_id, &item.id) {
                    tracing::debug!(item = %item.id, "ignored a stale item-open result");
                    return;
                }
                // The files of a pull request that is already showing. The
                // header, the conversation and the pane are left as they
                // are; what came with the checkout is filled in.
                if shown_early && review.is_item(&item.id) {
                    review.scope_gen.set(Some(scope_gen));
                    review.scope.set(scope);
                    review.set_changed_files(files);
                    review.tree.set(tree);
                    review.item.set(Some(item));
                    review.stale_head.set(stale_head);
                    review.pull_request.set(pull_request);
                    let paths = review.files_reload.try_update(std::mem::take).unwrap_or_default();
                    if !paths.is_empty() {
                        self.reload.update(|reload| reload.extend(paths));
                    }
                    // Last, after every field the editor reads as it is
                    // built in place of the progress.
                    review.files_pending.set(None);
                    return;
                }
                // The item's pane comes back unless it is the one open
                // already, whose pane is still on screen.
                let fresh = !review.is_item(&item.id);
                review.prepare_ready_for(&item.id);
                self.open_expectation
                    .set(OpenExpectation::Item(item.id.clone()));
                review.preparing.set(Preparing::Idle);
                review.scope_gen.set(Some(scope_gen));
                // A normal item open restores the live checkout after a
                // retained review snapshot was being inspected.
                review.clear_historical_review();
                review.scope.set(scope);
                review.set_changed_files(files);
                review.tree.set(tree);
                self.open_item.set(Some(item.id.clone()));
                let overview_loading = pull_request.is_some() && !offline;
                review.item.set(Some(item));
                review.stale_head.set(stale_head);
                review.offline.set(offline);
                review.upstream_changed.set(false);
                review.head_moved.set(None);
                review.pull_request.set(pull_request);
                review.pull_request_overview_loading.set(overview_loading);
                review.issue.set(None);
                review.issue_comments.set(Arc::new(Vec::new()));
                review.issue_events.set(Arc::new(Vec::new()));
                // The threads follow on their own event; whatever is here
                // is the last item's.
                review.comment_threads.set(Arc::new(Vec::new()));
                if fresh {
                    for path in review.restore_pane() {
                        self.reload.update(|reload| reload.push(path));
                    }
                }
                review.ready.set(true);
                review.set_phase(Phase::Ready);
                // Start work's branch opens with the issue in its composer,
                // for the reader to send or edit.
                let draft = self.start_work_draft.with_untracked(|d| {
                    d.as_ref()
                        .filter(|(id, _)| *id == navigation_id)
                        .map(|(_, text)| text.clone())
                });
                if let Some(draft) = draft {
                    self.start_work_draft.set(None);
                    if review.composer.with_untracked(Option::is_none) {
                        review.open_composer(None, None, draft, crate::review::threads::defaults(*self));
                    }
                }
            }
            // A pull request whose checkout is still being made: shown as an
            // item with no files yet, Code saying how the checkout is going.
            Event::PullRequestReady {
                navigation_id,
                item,
                pull_request,
            } => {
                if !self.accepts_open_item(navigation_id, &item.id) {
                    tracing::debug!(item = %item.id, "ignored a stale pull-request-open result");
                    return;
                }
                let fresh = !review.is_item(&item.id);
                review.prepare_ready_for(&item.id);
                self.open_expectation
                    .set(OpenExpectation::Item(item.id.clone()));
                let message = review.preparing.with_untracked(|p| match p {
                    Preparing::Working { message, .. } => message.clone(),
                    _ => String::new(),
                });
                review.preparing.set(Preparing::Idle);
                review.files_pending.set(Some(Preparing::Working {
                    message,
                    percent: None,
                }));
                review.clear_historical_review();
                review.set_changed_files(Arc::new(Vec::new()));
                review.tree.set(Arc::new(Vec::new()));
                self.open_item.set(Some(item.id.clone()));
                review.item.set(Some(item));
                review.stale_head.set(None);
                review.offline.set(false);
                review.upstream_changed.set(false);
                review.head_moved.set(None);
                review.pull_request_overview_loading.set(pull_request.is_open());
                review.pull_request.set(Some(pull_request));
                review.issue.set(None);
                review.issue_comments.set(Arc::new(Vec::new()));
                review.issue_events.set(Arc::new(Vec::new()));
                review.comment_threads.set(Arc::new(Vec::new()));
                // The tabs open last time come back now, and load once
                // there is a checkout to load them from.
                if fresh {
                    review.files_reload.set(review.restore_pane());
                }
                review.ready.set(true);
                review.set_phase(Phase::Ready);
            }
            Event::IssueReady {
                navigation_id,
                scope_gen,
                item,
                issue,
                comments,
                events,
                offline,
            } => {
                if !self.accepts_open_item(navigation_id, &item.id) {
                    tracing::debug!(item = %item.id, "ignored a stale issue-open result");
                    return;
                }
                let fresh = !review.is_item(&item.id);
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
                review.upstream_changed.set(false);
                review.head_moved.set(None);
                review.install_issue_reactions(&issue, &comments);
                if fresh {
                    review.issue_closable.set(None);
                }
                review.issue.set(Some(issue));
                review.issue_comments.set(comments);
                review.issue_events.set(events);
                review.comment_threads.set(Arc::new(Vec::new()));
                // An issue has no files, so its pane has nothing to load.
                if fresh {
                    review.restore_pane();
                }
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
                        if let Some(run_id) = pending.run_id {
                            review.pending_run_reveal.set(Some(run_id));
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
            Event::AgentSteer { run_id, text } => {
                self.append_open_agent_record(&run_id, perch_github::RunEvent::Steer { text: text.clone() });
                review.apply_steer(&run_id, &text);
            }
            Event::AgentRuns { runs, has_more } => self.merge_agent_runs(runs, has_more),
            Event::ItemRuns {
                item_id,
                runs,
                has_more,
            } => self.merge_item_runs(item_id, runs, has_more),
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
                    self.complain(message);
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
                    review
                        .lsp_status
                        .set(crate::review::model::ServerStatuses::default());
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
                    review.show_tab(ItemTab::Code);
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
                    self.complain(message);
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
            Event::ItemsPolled(items) => {
                if self.holds_items(&items) {
                    self.pending_items.set(Some(items));
                } else {
                    self.apply_with_review(Event::Items(items), review);
                }
            }
            Event::OpenItemChanged { item_id } => {
                if review.is_item(&item_id) {
                    review.upstream_changed.set(true);
                }
            }
            Event::ReviewFailed {
                navigation_id,
                message,
            } => {
                if !self.accepts_navigation(navigation_id) {
                    return;
                }
                // The checkout of a pull request already showing: Code says
                // why, and the conversation can still be read.
                if review.files_pending.with_untracked(Option::is_some) {
                    review.files_pending.set(Some(Preparing::Failed { message }));
                    return;
                }
                review.preparing.set(Preparing::Failed { message });
                review.set_phase(Phase::Failed);
            }
            Event::LspStatus {
                navigation_id,
                kind,
                status,
            } => {
                if self.accepts_navigation(navigation_id) {
                    // Before the status, so what shows a name asks again as
                    // the status reaches it.
                    if status == perch_lsp::Status::Ready {
                        review.forget_missing_names();
                    }
                    // Said once as it starts: the backend tells a status
                    // only when it changes, and a server that failed to
                    // load its workspace would otherwise look ready.
                    if let perch_lsp::Status::Failed { .. } | perch_lsp::Status::Unhealthy { .. } =
                        &status
                    {
                        if review.lsp_status.with_untracked(|s| s.get(kind) != Some(&status)) {
                            self.complain(status.describe(kind));
                        }
                    }
                    if let perch_lsp::Status::Missing(missing) = &status {
                        self.offer_server(ServerOffer::missing(missing, None), false);
                    }
                    let ready = status == perch_lsp::Status::Ready;
                    review.set_server_status(kind, status);
                    // After it, so the hover and the uses asked again are
                    // asked of a server that is ready.
                    if ready {
                        review.pending_ready.update(|kinds| kinds.push(kind));
                    }
                }
            }
            Event::LspInstalled { kind, install } => {
                // The open item's server was missing for want of this
                // install: start it now. One missing for another toolchain
                // stays missing.
                let wanted = review.lsp_status.with_untracked(|statuses| {
                    matches!(
                        statuses.get(kind),
                        Some(perch_lsp::Status::Missing(missing))
                            if missing.install.as_deref() == Some(&install)
                    )
                });
                if wanted {
                    review.set_server_status(kind, perch_lsp::Status::Starting);
                    review.pending_server_starts.update(|starts| starts.push(kind));
                }
                self.apply(Event::LspInstalled { kind, install });
            }
            Event::LspWarning {
                navigation_id,
                kind,
                message,
            } => {
                if self.accepts_navigation(navigation_id) {
                    // Its first line: rust-analyzer puts the tool output it
                    // failed on under it, which the log keeps.
                    let first = message.lines().next().unwrap_or(&message);
                    self.notify(format!("{}: {first}", kind.name()), Tone::Warning);
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
            Event::LspDocumentSymbols { path, outline } => {
                review.symbols.update(|symbols| {
                    symbols.insert(path, outline);
                });
            }
            Event::LspCallHierarchy { token, items, place } => {
                review.finish_prepare_hierarchy(token, items, place);
            }
            Event::LspNames { token, answers } => {
                review.finish_names(token, answers);
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
                events,
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
                    review.issue_events.set(events);
                }
            }
            Event::MentionableUsers {
                item_id,
                query,
                users,
            } => {
                if review.is_item(&item_id) {
                    review.mentions.install(query, users);
                }
            }
            Event::IssueReferences {
                item_id,
                query,
                references,
            } => {
                if review.is_item(&item_id) {
                    review.mentions.install_references(query, references);
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
            Event::ThreadResolveSettled { item_id, thread_id } => {
                if review.is_item(&item_id) {
                    review.settle_resolution(&thread_id);
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
            // rows go with it. The notice itself is the popup's.
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
                moved_to,
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
                    if moved_to.is_some() {
                        review.head_moved.set(moved_to);
                    }
                }
            }
            Event::PullRequestUpdated {
                item_id,
                item,
                pull_request,
                merge_attempt_id,
            } => {
                if review.is_item(&item_id) {
                    // The row's checkout is not the view's until the files
                    // it is waiting on arrive.
                    let item = if review.files_pending.with_untracked(Option::is_some) {
                        Arc::new(perch_github::Item {
                            checkout: None,
                            ..(*item).clone()
                        })
                    } else {
                        item
                    };
                    let moved = review.pull_request.with_untracked(|pull| {
                        pull.as_ref()
                            .is_some_and(|pull| pull.head_sha != pull_request.head_sha)
                    });
                    if moved {
                        review.head_moved.set(Some(pull_request.head_sha.clone()));
                    }
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
                posted,
                unmatched,
                failed,
                outside_diff,
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
                    let outcome = crate::review::model::submit_review_outcome(
                        submitted.is_some(),
                        posted,
                        &unmatched,
                        &failed,
                        &outside_diff,
                        &review.drafts(),
                    );
                    // Once something is up the form is done with, and
                    // what did not go is a warning in the popup: said in
                    // the form, under Submit, it read as a review that
                    // had failed. With nothing up the form stays to say
                    // why, or the popup does when the form is gone.
                    let went = submitted.is_some() || posted > 0;
                    let review_of = submitted.as_deref().cloned();
                    let in_form = outcome.clone().filter(|_| !went);
                    let owned = review.finish_submit_review(attempt_id, review_of, in_form);
                    if let Some(outcome) = outcome {
                        if went {
                            self.notify(outcome, Tone::Warning);
                        } else if !owned {
                            self.complain(outcome);
                        }
                    }
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
            // The comment itself came in a `DiscussionReady`, or an
            // `IssueComments` for an issue, before this.
            Event::IssueCommentPosted {
                item_id,
                token,
                result,
            } => {
                if review.is_item(&item_id) {
                    review.settle_issue_comment(token, result.map(|_| ()));
                }
            }
            // The closed pull request came in a `PullRequestUpdated`
            // before this.
            Event::PullRequestClosed {
                item_id,
                token,
                result,
            } => {
                if review.is_item(&item_id) {
                    review.settle_issue_comment(token, result);
                }
            }
            Event::SuggestionApplied {
                item_id,
                token,
                result,
            } => {
                if review.is_item(&item_id) {
                    review.settle_issue_comment(token, result);
                }
            }
            // A thread resolved after its suggestion is committed folds
            // as one resolved by hand does, until `ThreadResolveSettled`.
            Event::SuggestionCommitted {
                item_id,
                token,
                thread_id,
                result,
            } => {
                if review.is_item(&item_id) {
                    if result == Ok(true) {
                        review.begin_resolve(&thread_id, true);
                    }
                    review.settle_suggestion_commit(token, result.map(drop));
                }
            }
            // The closed issue came in an `IssueUpdated` before this.
            Event::IssueClosed {
                item_id,
                token,
                result,
            } => {
                if review.is_item(&item_id) {
                    review.settle_issue_comment(token, result);
                }
            }
            Event::IssueUpdated {
                item_id,
                item,
                issue,
            } => {
                let same_issue = review.issue.with_untracked(|current| {
                    current.as_ref().is_some_and(|current| {
                        current.repo_full_name == issue.repo_full_name
                            && current.number == issue.number
                    })
                });
                if review.is_item(&item_id) && same_issue {
                    review.item.set(Some(item));
                    review.issue.set(Some(issue));
                }
            }
            Event::IssueClosable {
                item_id,
                repo_full_name,
                number,
                closable,
            } => {
                let same_issue = review.issue.with_untracked(|current| {
                    current.as_ref().is_some_and(|current| {
                        current.repo_full_name == repo_full_name && current.number == number
                    })
                });
                if review.is_item(&item_id) && same_issue {
                    review.issue_closable.set(Some(closable));
                }
            }
            Event::CommitsReady {
                navigation_id,
                scope_gen,
                item_id,
                commits,
                dirt,
                since,
                last_run,
            } => {
                if self.accepts_navigation(navigation_id)
                    && review.is_item(&item_id)
                    && review.scope_gen.get_untracked() == Some(scope_gen)
                {
                    review.commits.set(commits);
                    review.dirty.set(dirt);
                    review.since_head.set(since);
                    review.last_run.set(last_run);
                }
            }
            Event::ScopeReady {
                navigation_id,
                scope_gen,
                item_id,
                scope,
                files,
                reread_all,
            } => {
                if self.accepts_navigation(navigation_id) && review.is_item(&item_id) {
                    review.scope_gen.set(Some(scope_gen));
                    let paths = if review.scope.with_untracked(|held| *held == scope) {
                        review.reread_scope(files, reread_all)
                    } else {
                        review.change_scope(scope, files)
                    };
                    self.reload.update(|r| r.extend(paths));
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
            Event::PullRequestReadiness { item_id, readiness } => {
                if review.is_item(&item_id) {
                    review.pull_readiness.set(Some(readiness));
                }
            }
            Event::PullRequestBases { item_id, bases } => {
                if review.is_item(&item_id) {
                    review.pull_bases.set(Some(bases));
                }
            }
            Event::OpenPullRequestStep { item_id, step } => {
                if review.is_item(&item_id) {
                    review.pull_step.set(Some(step));
                    review.pull_error.set(None);
                }
            }
            Event::OpenPullRequestFailed { item_id, message } => {
                if review.is_item(&item_id) {
                    review.pull_step.set(None);
                    review.pull_error.set(Some(message));
                }
            }
            Event::BranchPushed { item_id } => {
                if review.is_item(&item_id) {
                    review.push_pending.set(false);
                    review.push_error.set(None);
                }
            }
            Event::BranchPushFailed { item_id, message } => {
                if review.is_item(&item_id) {
                    review.push_pending.set(false);
                    review.push_error.set(Some(message));
                }
            }
            Event::PullRequestDescription { item_id, outcome } => {
                if review.is_item(&item_id) {
                    review.describing.set(false);
                    match &outcome {
                        crate::backend::DescriptionOutcome::Written { title, body }
                            if crate::review::open_pull::typed(*self, &item_id) =>
                        {
                            review.description_offer.set(Some((title.clone(), body.clone())));
                        }
                        crate::backend::DescriptionOutcome::Failed(message) => {
                            review.description_error.set(Some(message.clone()));
                        }
                        _ => {}
                    }
                }
                self.apply(Event::PullRequestDescription { item_id, outcome });
            }
            event @ (Event::FeedbackStep { .. }
            | Event::FeedbackReady(_)
            | Event::FeedbackSent { .. }
            | Event::FeedbackCaught { .. }
            | Event::FeedbackFailed(_)) => crate::ui::feedback::apply(*self, review, event),
            Event::IssueDraft { item_id, outcome } => {
                if review.is_item(&item_id) {
                    review.issue_drafting.set(false);
                    match &outcome {
                        crate::backend::DescriptionOutcome::Written { title, body }
                            if crate::review::close_as_issue::typed(*self, &item_id) =>
                        {
                            review.issue_draft_offer.set(Some((title.clone(), body.clone())));
                        }
                        crate::backend::DescriptionOutcome::Failed(message) => {
                            review.issue_draft_error.set(Some(message.clone()));
                        }
                        _ => {}
                    }
                }
                self.apply(Event::IssueDraft { item_id, outcome });
            }
            Event::ClosedAsIssue { item_id, result } => {
                if review.is_item(&item_id) {
                    review.issue_closing.set(false);
                    review.issue_error.set(result.as_ref().err().cloned());
                }
                self.apply(Event::ClosedAsIssue { item_id, result });
            }
            Event::PullRequestOpened {
                item_id,
                number,
                notice,
            } => {
                if review.is_item(&item_id) {
                    review.pull_step.set(None);
                    review.pull_error.set(None);
                }
                self.apply(Event::PullRequestOpened {
                    item_id,
                    number,
                    notice,
                });
            }
            Event::LspReferences { token, locations } => {
                review.finish_references(token, locations);
            }
            other => self.apply(other),
        }
    }

    /// Fold one event into what the sidebar and the popup read.
    pub fn apply(&self, event: Event) {
        match event {
            Event::LspUpdateAvailable {
                navigation_id,
                kind,
                text,
                install,
            } => {
                if self.accepts_navigation(navigation_id) {
                    self.offer_server(
                        ServerOffer {
                            kind,
                            key: text.clone(),
                            text,
                            install: Some(install),
                            action: "Update",
                        },
                        false,
                    );
                }
            }
            Event::LspInstalling { kind } => {
                self.installing_servers.update(|i| _ = i.insert(kind));
            }
            Event::LspInstalled { kind, .. } => {
                self.installing_servers.update(|i| _ = i.remove(&kind));
                self.drop_offers(kind);
            }
            Event::LspInstallFailed { kind, error } => {
                self.installing_servers.update(|i| _ = i.remove(&kind));
                self.drop_offers(kind);
                self.complain(format!("Couldn't install {}: {error}", kind.name()));
            }
            Event::Avatar { url, bytes } => {
                self.avatars.update(|m| {
                    m.insert(url, Arc::new(bytes));
                });
            }
            Event::Picture { url, decoded } => crate::review::images::arrived(url, decoded),
            Event::Participants(found) => {
                self.participants.update(|m| m.extend(found));
            }
            // Start work's prompt, held until its worktree opens.
            Event::StartWorkDraft { navigation_id, prompt } => {
                if self.accepts_navigation(navigation_id) {
                    self.start_work_draft.set(Some((navigation_id, prompt)));
                }
            }
            // What was typed went to GitHub, so the form's draft goes.
            Event::PullRequestOpened { item_id, notice, .. } => {
                self.pull_request_drafts.update(|drafts| {
                    drafts.remove(&item_id);
                });
                if let Some(notice) = notice {
                    self.complain(notice);
                }
            }
            // What the reader typed is never written over: the open item
            // asks first, and one not open keeps its text.
            Event::PullRequestDescription {
                item_id,
                outcome: crate::backend::DescriptionOutcome::Written { title, body },
            } => {
                if !crate::review::open_pull::typed(*self, &item_id) {
                    crate::review::open_pull::take_description(*self, &item_id, title, body);
                }
            }
            Event::PullRequestDescription { .. } => {}
            // The issue is open, so what was typed for it is spent.
            Event::ClosedAsIssue {
                item_id,
                result: Ok(_),
            } => {
                self.issue_drafts.update(|drafts| {
                    drafts.remove(&item_id);
                });
            }
            Event::ClosedAsIssue { .. } => {}
            Event::IssueDraft {
                item_id,
                outcome: crate::backend::DescriptionOutcome::Written { title, body },
            } => {
                if !crate::review::close_as_issue::typed(*self, &item_id) {
                    crate::review::close_as_issue::take_draft(*self, &item_id, title, body);
                }
            }
            Event::IssueDraft { .. } => {}
            // A window's own, applied with its review state.
            Event::FeedbackStep { .. }
            | Event::FeedbackReady(_)
            | Event::FeedbackSent { .. }
            | Event::FeedbackCaught { .. }
            | Event::FeedbackFailed(_) => {}
            // The rest are about the open item, and wait for no one.
            Event::PullRequestReadiness { .. }
            | Event::PullRequestBases { .. }
            | Event::OpenPullRequestStep { .. }
            | Event::OpenPullRequestFailed { .. }
            | Event::BranchPushed { .. }
            | Event::BranchPushFailed { .. } => {}
            // What the reader did sends the whole list, so whatever a poll
            // was holding back is in it.
            Event::Items(items) => {
                self.items.set(items);
                self.pending_items.set(None);
            }
            Event::ItemsPolled(items) => {
                if self.holds_items(&items) {
                    self.pending_items.set(Some(items));
                } else {
                    self.apply(Event::Items(items));
                }
            }
            Event::Notifications(notifications) => {
                self.notifications.set(notifications);
                self.pending_notifications.set(None);
            }
            Event::DoneNotifications(threads) => self.done_notifications.set(threads),
            Event::SnoozedNotifications(threads) => self.snoozed_notifications.set(threads),
            Event::NotificationsPolled(notifications) => {
                let first = !self.notifications_polled.get_untracked();
                self.notifications_polled.set(true);
                if !first && self.holds_notifications(&notifications) {
                    self.pending_notifications.set(Some(notifications));
                } else {
                    self.apply(Event::Notifications(notifications));
                }
            }
            Event::Repos(repos) => self.repos.set(repos),
            Event::GithubHosts(hosts) => self.github_hosts.set(hosts),
            Event::OpenReferenceResolved { navigation_id, kind } => {
                crate::ui::open_box::resolved(*self, navigation_id, kind)
            }
            Event::OpenReferenceFailed {
                navigation_id,
                message,
            } => crate::ui::open_box::failed(*self, navigation_id, message),
            Event::BrowseResults {
                query,
                page,
                poll,
                result,
            } => self.browse.update(|all| all.entry(query).or_default().take(page, poll, result)),
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
                "drawer.view" => self.tree_open.set(crate::ui::drawer::parse(&value)),
                "drawer.width" => match value.parse::<f64>() {
                    Ok(width) => self.drawer_width.set(width.clamp(
                        crate::theme::DRAWER_MIN_WIDTH,
                        crate::theme::DRAWER_MAX_WIDTH,
                    )),
                    Err(e) => tracing::warn!(error = %e, "ignoring saved drawer width"),
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
            Event::Notice(message) => self.complain(message),
            Event::Done(message) => self.notify(message, Tone::Success),
            Event::Bases { repo, bases } => self.new_work.take_bases(repo, bases),
            Event::WorkDefaults(work) => self.new_work.prefix.set(work.branch_prefix),
            Event::RepositoryAdded(identity) => self.added_repo.set(Some(identity)),
            Event::ApprovalRequested(view) => {
                if let Some(review) = self.review.get_untracked() {
                    review.approvals.update(|pending| {
                        if !pending.iter().any(|p| p.run_id == view.run_id && p.id == view.id) {
                            pending.push(view);
                        }
                    });
                }
            }
            Event::ApprovalSettled { run_id, approval_id } => {
                if let Some(review) = self.review.get_untracked() {
                    review
                        .approvals
                        .update(|pending| pending.retain(|p| !(p.run_id == run_id && p.id == approval_id)));
                }
            }
            Event::Grants { repo, grants } => {
                if let Some(review) = self.review.get_untracked() {
                    review.grants.update(|all| {
                        all.insert(repo.to_lowercase(), grants);
                    });
                }
            }
            Event::AssignableUsers {
                repo_full_name,
                users,
            } => {
                if let Some(review) = self.review.get_untracked() {
                    review.assignable.update(|all| {
                        all.insert(repo_full_name.to_lowercase(), users);
                    });
                }
            }
            Event::RunChanges { run_id, counts } => {
                if let Some(review) = self.review.get_untracked() {
                    review.run_changes.update(|changes| {
                        changes.insert(run_id, counts);
                    });
                }
            }
            Event::ConfirmLoss(prompt) => {
                self.renaming.set(None);
                self.confirm_delete.set(None);
                self.confirm_loss.set(Some(prompt));
            }
            Event::AgentConfig(agent) => self.agent.set(agent),
            Event::Usage(board) => self.usage.set(board),
            Event::CommentThreads { .. }
            | Event::AgentStep { .. }
            | Event::AgentSteer { .. }
            | Event::AgentRuns { .. }
            | Event::ItemRuns { .. }
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
            | Event::OpenItemChanged { .. }
            | Event::IssueReady { .. } => {}
            Event::ReviewProgress { .. }
            | Event::ReviewReady { .. }
            | Event::PullRequestReady { .. }
            | Event::ReviewFailed { .. }
            | Event::FileLoaded { .. }
            | Event::FileFailed { .. }
            | Event::LspStatus { .. }
            | Event::LspWarning { .. }
            | Event::LspHover { .. }
            | Event::LspDefinition { .. }
            | Event::LspTypeDefinition { .. }
            | Event::LspImplementation { .. }
            | Event::LspDocumentHighlight { .. }
            | Event::LspDocumentSymbols { .. }
            | Event::LspCallHierarchy { .. }
            | Event::LspCalls { .. }
            | Event::LspNames { .. }
            | Event::DiscussionReady { .. }
            | Event::IssueComments { .. }
            | Event::MentionableUsers { .. }
            | Event::IssueReferences { .. }
            | Event::ReactionsUpdated { .. }
            | Event::ReactionFailed { .. }
            | Event::ThreadResolveSettled { .. }
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
            | Event::IssueCommentPosted { .. }
            | Event::PullRequestClosed { .. }
            | Event::IssueClosed { .. }
            | Event::SuggestionApplied { .. }
            | Event::SuggestionCommitted { .. }
            | Event::IssueUpdated { .. }
            | Event::IssueClosable { .. }
            | Event::CommitsReady { .. }
            | Event::ScopeReady { .. }
            | Event::ViewedReady { .. }
            | Event::LspReferences { .. } => {}
            Event::Status(status) => {
                // Polls are quiet once one has gone through.
                if status == Status::Refreshing
                    && self.status.with_untracked(|s| matches!(s, Status::Idle { .. }))
                {
                    return;
                }
                if let Status::Idle { at, .. } = &status {
                    self.now.set(*at);
                    self.polls.update(|n| *n += 1);
                }
                // A failed poll is said once, when it starts failing or
                // fails another way; the next poll failing the same way
                // is not news. A missing token has a screen of its own.
                let said = |s: &Status| match s {
                    Status::Error { message } => Some((message.clone(), Tone::Error)),
                    Status::RateLimited { message } => Some((message.clone(), Tone::Warning)),
                    _ => None,
                };
                let last = self.last_poll_problem.get_untracked();
                let now = said(&status).map(|(message, _)| message);
                if status != Status::Refreshing {
                    if let Some((message, tone)) = said(&status).filter(|_| now != last) {
                        self.notify(message, tone);
                    }
                    self.last_poll_problem.set(now);
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
        self.new_work.open.set(false);
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
        self.new_work.open.set(false);
        review.begin(message);
    }

    /// The progress card, over the open review or the empty main area, for
    /// an act that ends in another item being opened: Start work.
    pub fn begin_open(&self, message: &str, navigation_id: u64) {
        self.expected_navigation_id.set(Some(navigation_id));
        self.pending_agent_navigation.set(None);
        self.agent_archived_owner.set(None);
        self.open_expectation.set(OpenExpectation::Any);
        self.open_item.set(None);
        self.new_work.open.set(false);
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

    /// Whether the inbox has an unread notification about the pull
    /// request or the issue: what bolds its row and draws its dot, in
    /// whichever scope the row is.
    pub fn subject_unread(
        &self,
        repo_full_name: &str,
        kind: perch_github::ItemKind,
        number: u64,
    ) -> bool {
        self.notifications.with(|threads| {
            threads
                .iter()
                .any(|thread| thread.unread && thread.is_about(repo_full_name, kind, number))
        })
    }

    /// Whether the main pane has the notification's pull request or issue
    /// open: what marking it Done closes. One still opening has no item
    /// yet, so it is not.
    pub fn is_viewing_thread(&self, thread: &Thread) -> bool {
        self.review.get_untracked().is_some_and(|review| {
            review.item.with_untracked(|item| {
                item.as_ref()
                    .and_then(|item| item.subject.as_ref())
                    .is_some_and(|subject| {
                        thread.is_about(subject.repo_full_name(), subject.kind(), subject.number())
                    })
            })
        })
    }

    /// Whether the drawer is showing local items: either of Worktrees'
    /// views.
    fn shows_items(&self) -> bool {
        self.item_filters.with_untracked(|f| f.tab == SidebarTab::Worktrees)
    }

    /// Whether a poll's items would change which rows the drawer shows,
    /// or their order. Changes inside a row go straight in; so does
    /// anything while the drawer shows no items, or nothing yet.
    fn holds_items(&self, incoming: &[Item]) -> bool {
        self.shows_items()
            && !self.items.with_untracked(Vec::is_empty)
            && self.items.with_untracked(|shown| self.visible_item_ids(shown))
                != self.visible_item_ids(incoming)
    }

    /// The same for notifications, while the drawer shows them.
    fn holds_notifications(&self, incoming: &[Thread]) -> bool {
        self.item_filters.with_untracked(|f| f.tab) == SidebarTab::Inbox
            && !self.notifications.with_untracked(Vec::is_empty)
            && self.notifications.with_untracked(|shown| self.visible_thread_ids(shown))
                != self.visible_thread_ids(incoming)
    }

    /// GitHub's threads under the drawer's filters: the inbox's, or under
    /// View › Done and View › Snoozed the ones put away so.
    pub fn visible_inbox(&self) -> Vec<Thread> {
        let (inbox, repository) = self.item_filters.with(|f| (f.inbox.clone(), f.repository.clone()));
        let query = self.item_query.get();
        let viewer = self.viewer.with(|viewer| viewer.as_ref().map(|(login, _)| login.clone()));
        self.inbox_source(inbox.view).with(|threads| {
            crate::inbox::filter(threads, &inbox, viewer.as_deref(), repository.as_deref(), &query)
        })
    }

    /// The list Inbox's View draws from.
    pub fn inbox_source(&self, view: InboxView) -> RwSignal<Vec<Thread>> {
        match view {
            InboxView::Done => self.done_notifications,
            InboxView::Snoozed => self.snoozed_notifications,
            InboxView::Inbox => self.notifications,
        }
    }

    /// The ids of the threads `threads` would give under the drawer's
    /// filters, in order. None under View › Done or Snoozed, which a poll
    /// of the inbox does not move.
    fn visible_thread_ids(&self, threads: &[Thread]) -> Vec<String> {
        let (inbox, repository) = self.item_filters.with_untracked(|f| (f.inbox.clone(), f.repository.clone()));
        if inbox.view.is_put_away() {
            return Vec::new();
        }
        let query = self.item_query.get_untracked();
        let viewer = self.viewer.with_untracked(|viewer| viewer.as_ref().map(|(login, _)| login.clone()));
        crate::inbox::filter(threads, &inbox, viewer.as_deref(), repository.as_deref(), &query)
            .into_iter()
            .map(|thread| thread.id)
            .collect()
    }

    /// Whether the inbox has an unread notification about the item's pull
    /// request or issue: what bolds its title, draws its dot, and keeps it
    /// under Unread Only. A branch with no subject has nothing to be
    /// unread.
    pub fn item_unread(&self, item: &Item) -> bool {
        item.subject
            .as_ref()
            .is_some_and(|subject| self.subject_unread(subject.repo_full_name(), subject.kind(), subject.number()))
    }

    /// The ids of the rows `items` would give on the drawer's tab, in
    /// order.
    fn visible_item_ids(&self, items: &[Item]) -> Vec<String> {
        let mut filters = self.item_filters.get_untracked();
        filters.query = self.item_query.get_untracked();
        let unread = |item: &Item| self.item_unread(item);
        crate::items::local_items(items, &filters, &unread).into_iter().map(|item| item.id).collect()
    }

    /// The search Browse is showing, if it is the tab and has one.
    pub fn browse_query(&self) -> Option<String> {
        let github = self.repos.with(|repos| crate::items::github_repositories(repos));
        self.item_filters.with(|f| f.browse_query(&github))
    }

    /// What the drawer's pill says while a poll is held back for the list
    /// it shows: how many rows are new, or that there is activity when
    /// the rows only moved. `None` when nothing is held back.
    pub fn held_back_news(&self) -> Option<String> {
        let (new, noun) = if let Some(query) = self.browse_query() {
            let held = self.browse.with(|all| all.get(&query).and_then(|page| page.held.clone()))?;
            let shown: HashSet<(String, u64)> = self.browse.with_untracked(|all| {
                all.get(&query)
                    .map(|page| page.hits.iter().map(hit_key).collect())
                    .unwrap_or_default()
            });
            (held.0.iter().filter(|hit| !shown.contains(&hit_key(hit))).count(), "result")
        } else if self.item_filters.with(|f| f.tab) == SidebarTab::Inbox {
            if self.item_filters.with(|f| f.inbox.view).is_put_away() {
                return None;
            }
            let incoming = self.pending_notifications.get()?;
            let shown: HashSet<String> = self
                .notifications
                .with_untracked(|shown| self.visible_thread_ids(shown))
                .into_iter()
                .collect();
            let ids = self.visible_thread_ids(&incoming);
            (ids.iter().filter(|id| !shown.contains(*id)).count(), "notification")
        } else {
            let incoming = self.pending_items.get()?;
            let shown: HashSet<String> = self
                .items
                .with_untracked(|shown| self.visible_item_ids(shown))
                .into_iter()
                .collect();
            let ids = self.visible_item_ids(&incoming);
            (ids.iter().filter(|id| !shown.contains(*id)).count(), "item")
        };
        Some(match new {
            0 => "New activity".to_string(),
            1 => format!("1 new {noun}"),
            n => format!("{n} new {noun}s"),
        })
    }

    /// Put up whatever the drawer's pill is holding back for the list it
    /// shows.
    pub fn show_held_back(&self) {
        if let Some(query) = floem::reactive::Effect::untrack(|| self.browse_query()) {
            self.browse.update(|all| {
                if let Some(page) = all.get_mut(&query) {
                    page.show_held();
                }
            });
        } else if self.item_filters.with_untracked(|f| f.tab) == SidebarTab::Inbox {
            self.show_pending_notifications();
        } else {
            self.show_pending_items();
        }
    }

    /// Put up the items a poll was holding back.
    fn show_pending_items(&self) {
        let Some(items) = self.pending_items.get_untracked() else {
            return;
        };
        match self.review.get_untracked() {
            Some(review) => self.apply_with_review(Event::Items(items), review),
            None => self.apply(Event::Items(items)),
        }
    }

    /// Put up the notifications a poll was holding back.
    fn show_pending_notifications(&self) {
        if let Some(threads) = self.pending_notifications.get_untracked() {
            self.apply(Event::Notifications(threads));
        }
    }

    /// The local rows the drawer shows on its tab, under its filters and
    /// search: Worktrees' view.
    pub fn visible_items(&self) -> Vec<Item> {
        let mut filters = self.item_filters.get();
        filters.query = self.item_query.get();
        let unread = |item: &Item| self.item_unread(item);
        self.items.with(|items| crate::items::local_items(items, &filters, &unread))
    }

    pub fn avatar_for(&self, url: &str) -> Option<Arc<Vec<u8>>> {
        if url.is_empty() {
            return None;
        }
        self.avatars.with(|m| m.get(url).cloned())
    }
}

/// A Browse view's results so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrowsePage {
    pub hits: Vec<perch_github::SearchHit>,
    /// Whether Show more has anything to show.
    pub more: bool,
    /// How many pages `hits` is.
    pub pages: u32,
    /// Why the last search didn't answer. The results before it stay.
    pub error: Option<String>,
    /// A refetch's first page that would move the rows, and its `more`,
    /// until the reader asks for it.
    pub held: Option<(Vec<perch_github::SearchHit>, bool)>,
}

impl BrowsePage {
    /// Take what a search answered. The first page replaces what is
    /// shown, unless a poll's refetch would add, remove or reorder the
    /// rows, which waits behind the pill; a change inside a row goes
    /// straight in. A later page is added when it is the next one.
    fn take(&mut self, page: u32, poll: bool, result: Result<perch_github::SearchPage, String>) {
        let found = match result {
            Ok(found) => found,
            Err(message) => {
                self.error = Some(message);
                return;
            }
        };
        self.error = None;
        if page == 1 {
            let first = (found.hits.len()).min(self.hits.len());
            let same_rows = self.hits.len() >= found.hits.len()
                && self.hits[..first].iter().map(hit_key).eq(found.hits.iter().map(hit_key));
            if poll && self.pages > 0 && !same_rows {
                self.held = Some((found.hits, found.more));
            } else if poll && same_rows && self.pages > 1 {
                self.hits.splice(..first, found.hits);
                self.held = None;
            } else {
                self.hits = found.hits;
                self.more = found.more;
                self.pages = 1;
                self.held = None;
            }
        } else if page == self.pages + 1 {
            let known: HashSet<(String, u64)> = self.hits.iter().map(hit_key).collect();
            self.hits.extend(found.hits.into_iter().filter(|hit| !known.contains(&hit_key(hit))));
            self.more = found.more;
            self.pages = page;
        }
    }

    /// Put up what a poll's refetch was holding back.
    fn show_held(&mut self) {
        if let Some((hits, more)) = self.held.take() {
            self.hits = hits;
            self.more = more;
            self.pages = 1;
        }
    }
}

/// A search result's identity: its repository and number.
fn hit_key(hit: &perch_github::SearchHit) -> (String, u64) {
    (hit.repo_full_name.to_lowercase(), hit.number)
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
            squash_message: Default::default(),
            merge_message: Default::default(),
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
            local_read: false,
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
            snoozed_until: None,
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
            scope: perch_git::Scope::Branch,
            files: Arc::new(Vec::new()),
            tree: Arc::new(Vec::new()),
            stale_head: None,
            offline: true,
            shown_early: false,
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
                instructions: None,
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
                worktree_parent_sha: None,
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

    fn showing_notifications(app: &AppState) {
        app.item_filters.update(|f| f.tab = SidebarTab::Inbox);
    }

    fn ids(threads: &[Thread]) -> Vec<&str> {
        threads.iter().map(|thread| thread.id.as_str()).collect()
    }

    #[test]
    fn the_first_poll_replaces_the_cached_inbox_outright() {
        let app = AppState::new();
        showing_notifications(&app);
        app.apply(Event::Notifications(vec![notification("1", true)]));
        app.apply(Event::NotificationsPolled(vec![
            notification("2", true),
            notification("1", true),
        ]));
        assert_eq!(ids(&app.notifications.get()), ["2", "1"]);
        assert_eq!(app.held_back_news(), None);
    }

    #[test]
    fn a_poll_that_would_move_the_inbox_waits_to_be_asked_for() {
        let app = AppState::new();
        showing_notifications(&app);
        app.apply(Event::NotificationsPolled(vec![notification("1", true)]));

        app.apply(Event::NotificationsPolled(vec![
            notification("3", true),
            notification("2", true),
            notification("1", true),
        ]));
        assert_eq!(ids(&app.notifications.get()), ["1"]);
        assert_eq!(app.held_back_news().as_deref(), Some("2 new notifications"));

        app.show_held_back();
        assert_eq!(ids(&app.notifications.get()), ["3", "2", "1"]);
        assert_eq!(app.held_back_news(), None);
    }

    #[test]
    fn a_poll_that_only_changes_rows_in_place_goes_straight_in() {
        let app = AppState::new();
        showing_notifications(&app);
        app.apply(Event::NotificationsPolled(vec![notification("1", true)]));
        app.apply(Event::NotificationsPolled(vec![notification("1", false)]));
        assert!(!app.notifications.get()[0].unread);
        assert_eq!(app.held_back_news(), None);
    }

    #[test]
    fn a_reordered_inbox_is_news_without_new_rows() {
        let app = AppState::new();
        showing_notifications(&app);
        app.apply(Event::NotificationsPolled(vec![
            notification("1", true),
            notification("2", true),
        ]));
        app.apply(Event::NotificationsPolled(vec![
            notification("2", true),
            notification("1", true),
        ]));
        assert_eq!(ids(&app.notifications.get()), ["1", "2"]);
        assert_eq!(app.held_back_news().as_deref(), Some("New activity"));
    }

    #[test]
    fn what_the_reader_does_sends_the_whole_inbox_and_ends_the_wait() {
        let app = AppState::new();
        showing_notifications(&app);
        app.apply(Event::NotificationsPolled(vec![notification("1", true)]));
        app.apply(Event::NotificationsPolled(vec![
            notification("2", true),
            notification("1", true),
        ]));
        app.apply(Event::Notifications(vec![
            notification("2", true),
            notification("1", false),
        ]));
        assert_eq!(ids(&app.notifications.get()), ["2", "1"]);
        assert_eq!(app.held_back_news(), None);
    }

    #[test]
    fn a_poll_of_a_list_not_on_screen_goes_straight_in() {
        let app = AppState::new();
        app.item_filters.update(|f| f.tab = SidebarTab::Worktrees);
        app.apply(Event::NotificationsPolled(vec![notification("1", true)]));
        app.apply(Event::NotificationsPolled(vec![
            notification("2", true),
            notification("1", true),
        ]));
        assert_eq!(ids(&app.notifications.get()), ["2", "1"]);
    }

    fn hit(number: u64, title: &str) -> perch_github::SearchHit {
        perch_github::SearchHit {
            kind: perch_github::ItemKind::PullRequest,
            repo_full_name: "a/b".into(),
            number,
            title: title.into(),
            state: "open".into(),
            state_reason: None,
            draft: false,
            merged: false,
            author_login: "u".into(),
            author_avatar_url: String::new(),
            updated_at: "2026-09-20T10:00:00Z".parse().unwrap(),
            html_url: String::new(),
        }
    }

    fn found(hits: Vec<perch_github::SearchHit>, more: bool) -> Result<perch_github::SearchPage, String> {
        Ok(perch_github::SearchPage { hits, more })
    }

    fn numbers(page: &BrowsePage) -> Vec<u64> {
        page.hits.iter().map(|hit| hit.number).collect()
    }

    /// A view's first page goes straight in; a poll's refetch that would
    /// move the rows waits behind the pill, and one that changes only
    /// inside a row doesn't; Show more adds the next page; a failure
    /// keeps what was there.
    #[test]
    fn a_browse_view_holds_back_a_refetch_that_moves_its_rows() {
        let mut page = BrowsePage::default();
        page.take(1, false, found(vec![hit(1, "one"), hit(2, "two")], true));
        assert_eq!(numbers(&page), [1, 2]);
        assert!(page.more);

        page.take(2, false, found(vec![hit(3, "three")], false));
        assert_eq!(numbers(&page), [1, 2, 3]);
        assert_eq!(page.pages, 2);
        page.take(2, false, found(vec![hit(9, "late")], false));
        assert_eq!(numbers(&page), [1, 2, 3], "a page already taken is not taken again");

        page.take(1, true, found(vec![hit(1, "one, renamed"), hit(2, "two")], true));
        assert_eq!(page.hits[0].title, "one, renamed", "a change inside a row goes in");
        assert_eq!(numbers(&page), [1, 2, 3], "and the later page stays");
        assert!(page.held.is_none());

        page.take(1, true, found(vec![hit(4, "four"), hit(1, "one"), hit(2, "two")], true));
        assert_eq!(numbers(&page), [1, 2, 3], "the rows stay put");
        assert!(page.held.is_some());
        page.show_held();
        assert_eq!(numbers(&page), [4, 1, 2]);
        assert_eq!(page.pages, 1);

        page.take(1, false, Err("Offline".into()));
        assert_eq!(page.error.as_deref(), Some("Offline"));
        assert_eq!(numbers(&page), [4, 1, 2], "the last results stay");
        page.take(1, false, found(vec![hit(5, "five")], false));
        assert!(page.error.is_none());
        assert_eq!(numbers(&page), [5], "picking the view again takes what it finds");
    }

    #[test]
    fn the_pill_counts_new_browse_results() {
        let app = AppState::new();
        app.item_filters.update(|f| {
            f.tab = SidebarTab::Browse;
            f.browse.view = crate::items::BrowseView::Yours;
        });
        let query = app.browse_query().unwrap();
        let result = |hits, poll| Event::BrowseResults { query: query.clone(), page: 1, poll, result: found(hits, false) };
        app.apply(result(vec![hit(1, "one")], false));
        assert_eq!(app.held_back_news(), None);
        app.apply(result(vec![hit(2, "two"), hit(1, "one")], true));
        assert_eq!(app.held_back_news().as_deref(), Some("1 new result"));
        app.show_held_back();
        assert_eq!(app.held_back_news(), None);
        assert_eq!(app.browse.with(|all| numbers(&all[&query])), [2, 1]);
    }

    #[test]
    fn a_poll_with_new_items_waits_to_be_asked_for() {
        let item = |id: &str| {
            let mut item = Item::new("a/b", "2026-09-18T09:59:00Z".parse().unwrap());
            item.id = id.into();
            item
        };
        let app = AppState::new();
        // Worktrees › Recent, where items with no worktree are.
        app.item_filters.update(|f| {
            f.tab = SidebarTab::Worktrees;
            f.worktrees.view = crate::items::WorktreeView::Recent;
        });
        app.apply(Event::Items(vec![item("a")]));

        app.apply(Event::ItemsPolled(vec![item("a"), item("b")]));
        assert_eq!(app.items.get().len(), 1);
        assert_eq!(app.held_back_news().as_deref(), Some("1 new item"));

        app.show_held_back();
        assert_eq!(app.items.get().len(), 2);
        assert_eq!(app.held_back_news(), None);

        // A change inside a row moves nothing.
        let mut renamed = item("a");
        renamed.name = Some("renamed".into());
        app.apply(Event::ItemsPolled(vec![renamed, item("b")]));
        assert_eq!(app.items.get()[0].name.as_deref(), Some("renamed"));
        assert_eq!(app.held_back_news(), None);
    }

    #[test]
    fn news_about_the_open_item_is_flagged_until_it_opens_again() {
        let app = AppState::new();
        let review = review_of("open");
        app.apply_with_review(
            Event::OpenItemChanged {
                item_id: "other".into(),
            },
            review,
        );
        assert!(!review.upstream_changed.get());
        app.apply_with_review(
            Event::OpenItemChanged {
                item_id: "open".into(),
            },
            review,
        );
        assert!(review.upstream_changed.get());
        app.begin_item_review(review, "Reading the discussion", "open", 3);
        app.apply_with_review(review_ready("open", 3), review);
        assert!(!review.upstream_changed.get());
    }

    #[test]
    fn a_poll_after_one_has_gone_through_leaves_the_status_alone() {
        let app = AppState::new();
        let at = "2026-09-19T12:00:00Z".parse().unwrap();
        let idle = Status::Idle {
            at,
            source: "gh".into(),
        };
        app.apply(Event::Status(idle.clone()));
        app.apply(Event::Status(Status::Refreshing));
        assert_eq!(app.status.get(), idle);
    }

    #[test]
    fn done_closes_only_the_pane_showing_the_notifications_subject() {
        let issue = |repo: &str, number: u64| {
            let now = "2026-09-19T12:00:00Z".parse().unwrap();
            let mut item = Item::new(repo, now);
            item.subject = Some(perch_github::ItemSubject::Issue(perch_github::Issue {
                node_id: String::new(),
                reactions: Default::default(),
                number,
                title: format!("Issue {number}"),
                body: String::new(),
                state: "open".into(),
                state_reason: None,
                author_login: "u".into(),
                author_avatar_url: String::new(),
                labels: vec![],
                assignees: vec![],
                repo_full_name: repo.into(),
                comments: 0,
                created_at: Some(now),
                updated_at: now,
            }));
            Arc::new(item)
        };
        let app = AppState::new();
        let review = ReviewState::new();
        app.review.set(Some(review));
        let thread = notification("7", true);

        assert!(!app.is_viewing_thread(&thread));
        review.item.set(Some(issue("org/repo", 7)));
        assert!(app.is_viewing_thread(&thread));
        review.item.set(Some(issue("org/repo", 8)));
        assert!(!app.is_viewing_thread(&thread));
        review.item.set(Some(issue("org/other", 7)));
        assert!(!app.is_viewing_thread(&thread));
        // Still opening: no item yet, so nothing to close.
        review.item.set(Some(issue("org/repo", 7)));
        review.begin("Reading the notification");
        assert!(!app.is_viewing_thread(&thread));
    }

    /// A pull request shown before its checkout: the review is up with its
    /// header and conversation, the checkout's progress and failure go to
    /// Code rather than over the pane, and the files fill in without
    /// taking the rest down.
    #[test]
    fn a_pull_request_shown_before_its_checkout_takes_its_files_in_code() {
        use crate::review::model::{Phase, Preparing};
        let app = AppState::new();
        let review = ReviewState::new();
        app.begin_item_review(review, "Reading the pull request", "pr", 40);
        let mut item = Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
        item.id = "pr".into();
        let pull = pull_request(7, "h", "open");
        app.apply_with_review(
            Event::PullRequestReady {
                navigation_id: 40,
                item: Arc::new(item.clone()),
                pull_request: Arc::new(pull),
            },
            review,
        );
        assert_eq!(review.phase.get(), Phase::Ready);
        assert!(review.ready.get());
        assert!(review.pull_request.get().is_some());
        assert!(review.has_files(), "Code is there to show the checkout coming");

        app.apply_with_review(
            Event::ReviewProgress {
                navigation_id: 40,
                message: "Fetching the pull request".into(),
                percent: None,
            },
            review,
        );
        assert_eq!(review.phase.get(), Phase::Ready, "the conversation stays up");
        assert!(matches!(
            review.files_pending.get(),
            Some(Preparing::Working { message, .. }) if message == "Fetching the pull request"
        ));

        let mut ready = review_ready("pr", 40);
        if let Event::ReviewReady { shown_early, .. } = &mut ready {
            *shown_early = true;
        }
        app.apply_with_review(ready, review);
        assert_eq!(review.files_pending.get(), None);
        assert_eq!(review.phase.get(), Phase::Ready);
        assert!(review.is_item("pr"));
    }

    /// The checkout of a pull request already showing could not be made:
    /// Code says why, and the conversation can still be read.
    #[test]
    fn a_checkout_failing_under_a_shown_pull_request_leaves_it_up() {
        use crate::review::model::{Phase, Preparing};
        let app = AppState::new();
        let review = ReviewState::new();
        app.begin_item_review(review, "Reading the pull request", "pr", 41);
        review.files_pending.set(Some(Preparing::Working {
            message: String::new(),
            percent: None,
        }));
        review.set_phase(Phase::Ready);
        app.apply_with_review(
            Event::ReviewFailed {
                navigation_id: 41,
                message: "no such ref".into(),
            },
            review,
        );
        assert_eq!(review.phase.get(), Phase::Ready);
        assert_eq!(
            review.files_pending.get(),
            Some(Preparing::Failed {
                message: "no such ref".into()
            })
        );
    }

    /// A run on another item: the item opens on Conversation, whatever it
    /// was left on, and the run is asked for in its Agents section once its
    /// threads are here.
    #[test]
    fn a_run_on_another_item_opens_it_on_conversation_asking_for_the_run() {
        use crate::review::model::ItemTab;
        let app = AppState::new();
        let review = ReviewState::new();
        app.begin_item_review(review, "Open", "other", 30);
        app.apply_with_review(review_ready("other", 30), review);
        review.show_tab(ItemTab::Code);
        app.begin_item_review(review, "Open", "here", 31);
        app.apply_with_review(review_ready("here", 31), review);

        app.begin_agent_run_navigation("other".into(), "run-7".into(), 32);
        app.begin_review(review, "Reading the discussion");
        app.apply_with_review(review_ready("other", 32), review);
        assert_eq!(review.pending_run_reveal.get(), None, "asked before the threads are here");
        app.apply_with_review(
            Event::CommentThreads {
                item_id: "other".into(),
                threads: Vec::new(),
                review_group_id: None,
                review_request_id: None,
            },
            review,
        );
        assert!(review.is_item("other"));
        assert_eq!(review.tab.get(), ItemTab::Conversation);
        assert_eq!(review.pending_run_reveal.get().as_deref(), Some("run-7"));
        assert_eq!(review.focus_card.get(), None);
    }

    #[test]
    fn a_late_agent_view_cannot_replace_or_focus_over_the_latest_target() {
        let app = AppState::new();
        let review = ReviewState::new();

        let first = app.begin_agent_run_navigation("older".into(), "run-old".into(), 11);
        let latest = app.begin_agent_run_navigation("latest".into(), "run-new".into(), 12);
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
        assert_eq!(review.pending_run_reveal.get().as_deref(), Some("run-new"));

        // The first load can still finish after the latest one. It remains
        // inert, and closing the workspace also fences delayed Ready events.
        app.apply_with_review(review_ready("older", 11), review);
        assert!(review.is_item("latest"));
        app.leave_review(review);
        app.apply_with_review(review_ready("latest", 12), review);
        assert!(!review.is_showing());
    }

    /// Start work's branch opens with the prompt in its composer, once:
    /// a later open of the same branch starts empty. A prompt from a
    /// Start work since overtaken is not kept.
    #[test]
    fn start_work_leaves_its_prompt_in_the_new_branchs_composer() {
        let app = AppState::new();
        let review = ReviewState::new();
        app.review.set(Some(review));

        app.begin_open("Cutting a branch", 40);
        app.apply(Event::StartWorkDraft {
            navigation_id: 39,
            prompt: "Work on #6".into(),
        });
        assert!(app.start_work_draft.get().is_none());
        app.apply(Event::StartWorkDraft {
            navigation_id: 40,
            prompt: "Work on #7".into(),
        });
        app.apply_with_review(review_ready("branch", 40), review);
        assert!(review.composer.get().is_some());
        assert_eq!(review.composer_draft.get(), "Work on #7");
        assert!(app.start_work_draft.get().is_none());

        review.close_composer();
        app.begin_item_review(review, "Opening", "branch", 41);
        app.apply_with_review(review_ready("branch", 41), review);
        assert!(review.composer.get().is_none());
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
        review.set_server_status(perch_lsp::ServerKind::RustAnalyzer, perch_lsp::Status::Ready);
        app.expect_item_navigation("new", 41);

        app.apply_with_review(review_ready("new", 41), review);

        assert!(review.is_item("new"));
        assert!(review.tabs.with(Vec::is_empty));
        assert!(review.loading.with(HashMap::is_empty));
        assert_eq!(review.lsp_status.get(), Default::default());
    }

    /// An item opened again comes back on the pane it was left with, and
    /// the files its tabs hold are asked for, the one showing first. One
    /// opened for the first time is on Conversation.
    #[test]
    fn an_item_opened_again_comes_back_on_its_pane_and_asks_for_its_files() {
        use crate::review::model::ItemTab;
        let app = AppState::new();
        let review = ReviewState::new();

        app.begin_item_review(review, "Open", "a", 71);
        app.apply_with_review(review_ready("a", 71), review);
        assert_eq!(review.tab.get(), ItemTab::Conversation);
        review.open_tab("src/x.rs");
        review.open_tab("src/y.rs");
        review.show_tab(ItemTab::Code);

        app.begin_item_review(review, "Open", "b", 72);
        app.apply_with_review(review_ready("b", 72), review);
        assert_eq!(review.tab.get(), ItemTab::Conversation);
        assert!(review.tabs.with(Vec::is_empty));
        assert!(app.reload.get().is_empty());

        app.begin_item_review(review, "Open", "a", 73);
        app.apply_with_review(review_ready("a", 73), review);
        assert_eq!(review.tab.get(), ItemTab::Code);
        assert_eq!(review.tabs.with(Vec::len), 2);
        assert_eq!(review.active.get().as_deref(), Some("src/y.rs"));
        assert_eq!(app.reload.get(), ["src/y.rs", "src/x.rs"]);
    }

    /// A ready for the item already open, as agent navigation within it
    /// sends, leaves its pane as it is on screen.
    #[test]
    fn a_ready_for_the_item_already_open_keeps_its_pane() {
        use crate::review::model::ItemTab;
        let app = AppState::new();
        let review = review_of("same");
        review.open_tab("src/x.rs");
        review.show_tab(ItemTab::Code);
        app.expect_item_navigation("same", 43);

        app.apply_with_review(review_ready("same", 43), review);

        assert_eq!(review.tab.get(), ItemTab::Code);
        assert_eq!(review.active.get().as_deref(), Some("src/x.rs"));
        assert!(app.reload.get().is_empty());
    }

    #[test]
    fn an_accepted_same_item_ready_fences_the_old_lsp_status() {
        let app = AppState::new();
        let review = review_of("same");
        review.set_server_status(perch_lsp::ServerKind::RustAnalyzer, perch_lsp::Status::Ready);
        app.expect_item_navigation("same", 42);

        app.apply_with_review(review_ready("same", 42), review);

        assert_eq!(review.lsp_status.get(), Default::default());
    }

    fn missing_rust() -> perch_lsp::Missing {
        perch_lsp::Missing {
            kind: perch_lsp::ServerKind::RustAnalyzer,
            needs: None,
            install: Some(Box::new(perch_lsp::Install::Command {
                program: "rustup".into(),
                args: vec!["component".into(), "add".into(), "rust-analyzer".into()],
                env: Vec::new(),
                into: None,
            })),
            toolchain: Some("1.98.1-aarch64-apple-darwin".into()),
        }
    }

    fn offers(app: &AppState) -> Vec<ServerOffer> {
        app.toasts.with(|toasts| {
            toasts
                .iter()
                .filter_map(|toast| match &toast.kind {
                    ToastKind::Server(offer) => Some(offer.clone()),
                    _ => None,
                })
                .collect()
        })
    }

    /// A missing server is offered once, stays offered under other news,
    /// and once closed stays closed until a goto asks for it.
    #[test]
    fn a_missing_server_is_offered_until_it_is_turned_down() {
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        let review = ReviewState::new();
        app.begin_item_review(review, "Open", "a", 91);
        app.apply_with_review(review_ready("a", 91), review);
        let missing = || Event::LspStatus {
            navigation_id: 91,
            kind: perch_lsp::ServerKind::RustAnalyzer,
            status: perch_lsp::Status::Missing(missing_rust()),
        };
        app.apply_with_review(missing(), review);
        let offered = offers(&app);
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].text, "rust-analyzer isn't installed for Rust 1.98.1.");
        assert_eq!(offered[0].action, "Install");
        app.apply_with_review(missing(), review);
        assert_eq!(offers(&app).len(), 1, "the same offer is up once");

        // News goes over it, and it comes back when the news goes.
        app.complain("Could not post");
        assert_eq!(app.notice().as_deref(), Some("Could not post"));
        let news = app.current_toast().unwrap().token;
        app.dismiss_toast(news);
        assert!(matches!(app.current_toast().unwrap().kind, ToastKind::Server(_)));

        let token = app.current_toast().unwrap().token;
        app.decline_offer(token, &offered[0].key);
        assert!(offers(&app).is_empty());
        app.apply_with_review(missing(), review);
        assert!(offers(&app).is_empty(), "closed for the session");

        // A goto that wanted the server offers it again, with its line.
        app.offer_server(
            ServerOffer::missing(&missing_rust(), Some("No definition found for `x`: …".into())),
            true,
        );
        assert_eq!(offers(&app)[0].text, "No definition found for `x`: …");
    }

    /// An out-of-date install of Perch's is offered its update once, not
    /// again for each start, and not for a navigation since left.
    #[test]
    fn an_old_install_is_offered_its_update_once() {
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        let review = ReviewState::new();
        app.begin_item_review(review, "Open", "a", 93);
        app.apply_with_review(review_ready("a", 93), review);
        let update = |navigation_id| Event::LspUpdateAvailable {
            navigation_id,
            kind: perch_lsp::ServerKind::Gopls,
            text: "gopls 0.23.0 is available.".into(),
            install: perch_lsp::Install::Command {
                program: "go".into(),
                args: vec!["install".into()],
                env: Vec::new(),
                into: None,
            },
        };
        app.apply_with_review(update(92), review);
        assert!(offers(&app).is_empty(), "a navigation since left");
        app.apply_with_review(update(93), review);
        app.apply_with_review(update(93), review);
        let offered = offers(&app);
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].action, "Update");
        assert_eq!(offered[0].text, "gopls 0.23.0 is available.");
        let token = app.current_toast().unwrap().token;
        app.decline_offer(token, &offered[0].key);
        app.apply_with_review(update(93), review);
        assert!(offers(&app).is_empty(), "turned down for the session");
    }

    /// Every window hears an install run; the one whose server was missing
    /// for want of it starts the server, and a failure says why.
    #[test]
    fn an_installed_server_starts_where_it_was_missing() {
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        let review = ReviewState::new();
        app.begin_item_review(review, "Open", "a", 92);
        app.apply_with_review(review_ready("a", 92), review);
        let kind = perch_lsp::ServerKind::RustAnalyzer;
        app.apply_with_review(
            Event::LspStatus {
                navigation_id: 92,
                kind,
                status: perch_lsp::Status::Missing(missing_rust()),
            },
            review,
        );
        app.apply_with_review(Event::LspInstalling { kind }, review);
        assert!(app.installing_servers.get().contains(&kind));

        let mut other = missing_rust();
        other.install = None;
        app.apply_with_review(
            Event::LspInstalled {
                kind,
                install: perch_lsp::Install::Command {
                    program: "rustup".into(),
                    args: vec!["something".into(), "else".into()],
                    env: Vec::new(),
                    into: None,
                },
            },
            review,
        );
        assert!(review.pending_server_starts.get().is_empty(), "another toolchain's install");
        assert!(offers(&app).is_empty());

        app.apply_with_review(
            Event::LspInstalled {
                kind,
                install: *missing_rust().install.unwrap(),
            },
            review,
        );
        assert!(!app.installing_servers.get().contains(&kind));
        assert_eq!(review.pending_server_starts.get(), [kind]);
        assert_eq!(review.lsp_status.get().get(kind), Some(&perch_lsp::Status::Starting));

        app.apply_with_review(
            Event::LspInstallFailed {
                kind,
                error: "no network".into(),
            },
            review,
        );
        assert_eq!(app.notice().as_deref(), Some("Couldn't install rust-analyzer: no network"));
    }

    #[test]
    fn a_server_that_cannot_load_its_workspace_says_so_once() {
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        let review = ReviewState::new();
        app.begin_item_review(review, "Open", "a", 81);
        app.apply_with_review(review_ready("a", 81), review);
        let status = |status| Event::LspStatus {
            navigation_id: 81,
            kind: perch_lsp::ServerKind::RustAnalyzer,
            status,
        };
        let unhealthy = perch_lsp::Status::Unhealthy {
            message: "Failed to load workspaces\nUsage: cargo metadata".into(),
        };

        app.apply_with_review(status(perch_lsp::Status::Ready), review);
        assert_eq!(app.notice(), None);
        // Ready: what came back empty is asked again by the root view.
        assert_eq!(review.pending_ready.get(), [perch_lsp::ServerKind::RustAnalyzer]);
        review.pending_ready.set(Vec::new());
        app.apply_with_review(status(unhealthy.clone()), review);
        assert!(review.pending_ready.get().is_empty());
        assert_eq!(
            app.notice().as_deref(),
            Some("rust-analyzer: Failed to load workspaces")
        );
        assert!(!review.lsp_status.get().usable_for("src/lib.rs"));

        // The same again is no news.
        app.notify("", Tone::Error);
        app.apply_with_review(status(unhealthy), review);
        assert_eq!(app.notice(), None);

        // A warning leaves the server usable and says its first line.
        app.apply_with_review(status(perch_lsp::Status::Ready), review);
        app.apply_with_review(
            Event::LspWarning {
                navigation_id: 81,
                kind: perch_lsp::ServerKind::RustAnalyzer,
                message: "Failed to read Cargo metadata: unexpected argument\n\nUsage: cargo"
                    .into(),
            },
            review,
        );
        assert_eq!(
            app.notice().as_deref(),
            Some("rust-analyzer: Failed to read Cargo metadata: unexpected argument")
        );
        assert!(review.lsp_status.get().usable_for("src/lib.rs"));

        // One from a checkout since left says nothing.
        app.notify("", Tone::Error);
        app.apply_with_review(
            Event::LspWarning {
                navigation_id: 80,
                kind: perch_lsp::ServerKind::RustAnalyzer,
                message: "stale".into(),
            },
            review,
        );
        assert_eq!(app.notice(), None);
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

    /// A new list under the scope held, as a look after an agent's edit
    /// sends, asks again for the loaded files whose entry changed and
    /// leaves them showing until their new sides arrive. One whose entry
    /// is the same is kept as it is; with the head moved, it is asked for
    /// too. Another scope forgets them at once.
    #[test]
    fn a_reread_scope_reloads_the_files_whose_entry_changed_and_keeps_them_showing() {
        let app = AppState::new();
        let review = ReviewState::new();

        app.begin_item_review(review, "Open", "same", 81);
        app.apply_with_review(review_ready_at_scope("same", 81, 810), review);
        let loaded = |path: &str, text: &str| {
            Arc::new(crate::backend::build_loaded_file(
                path.into(),
                text.into(),
                "old\n".into(),
                Some(perch_git::ChangeStatus::Modified),
                false,
                perch_git::Scope::Branch,
                String::new(),
            ))
        };
        let entry = |path: &str, blob: &str| perch_git::ChangedFile {
            path: path.into(),
            old_path: None,
            status: perch_git::ChangeStatus::Modified,
            additions: 1,
            deletions: 1,
            binary: false,
            head_blob: Some(blob.into()),
        };
        let ready = |scope_gen, scope, files: Vec<perch_git::ChangedFile>, reread_all| {
            Event::ScopeReady {
                navigation_id: 81,
                scope_gen,
                item_id: "same".into(),
                scope,
                files: Arc::new(files),
                reread_all,
            }
        };
        let file_loaded = |scope_gen, path: &str, text: &str| Event::FileLoaded {
            navigation_id: 81,
            scope_gen,
            item_id: "same".into(),
            path: path.into(),
            file: loaded(path, text),
        };
        review.finish_load("src/x.rs", loaded("src/x.rs", "before\n"));
        review.finish_load("src/y.rs", loaded("src/y.rs", "same\n"));
        let _ = app.reload.try_update(std::mem::take);

        // Both enter the list: both are asked for again.
        app.apply_with_review(
            ready(811, perch_git::Scope::Branch, vec![entry("src/x.rs", "x1"), entry("src/y.rs", "y1")], false),
            review,
        );
        let mut asked = app.reload.try_update(std::mem::take).unwrap();
        asked.sort();
        assert_eq!(asked, ["src/x.rs", "src/y.rs"]);
        assert!(review.is_loading("src/x.rs"));
        assert!(review.file("src/x.rs").is_some(), "it shows while it reloads");
        app.apply_with_review(file_loaded(811, "src/x.rs", "after\n"), review);
        app.apply_with_review(file_loaded(811, "src/y.rs", "same\n"), review);
        assert!(!review.is_loading("src/x.rs"));
        assert!(review.file("src/x.rs").unwrap().buffer.text.contains("after"));

        // Only x's blob moved: y is kept, the very file it was.
        let y = review.file("src/y.rs").unwrap();
        app.apply_with_review(
            ready(812, perch_git::Scope::Branch, vec![entry("src/x.rs", "x2"), entry("src/y.rs", "y1")], false),
            review,
        );
        assert_eq!(app.reload.try_update(std::mem::take).unwrap(), ["src/x.rs"]);
        assert!(!review.is_loading("src/y.rs"));
        assert!(Arc::ptr_eq(&y, &review.file("src/y.rs").unwrap()));

        // x's answer for 812 never came; the next list asks again though
        // its entry is the same, since that answer would be dropped.
        app.apply_with_review(
            ready(813, perch_git::Scope::Branch, vec![entry("src/x.rs", "x2"), entry("src/y.rs", "y1")], false),
            review,
        );
        assert_eq!(app.reload.try_update(std::mem::take).unwrap(), ["src/x.rs"]);
        app.apply_with_review(file_loaded(813, "src/x.rs", "after\n"), review);

        // The head moved: everything is read again.
        app.apply_with_review(
            ready(814, perch_git::Scope::Branch, vec![entry("src/x.rs", "x2"), entry("src/y.rs", "y1")], true),
            review,
        );
        let mut asked = app.reload.try_update(std::mem::take).unwrap();
        asked.sort();
        assert_eq!(asked, ["src/x.rs", "src/y.rs"]);

        app.apply_with_review(ready(815, perch_git::Scope::Uncommitted, Vec::new(), true), review);
        assert!(review.file("src/x.rs").is_none());
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
                reread_all: true,
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
                reread_all: true,
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
                last_run: crate::backend::LastRun::None,
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
                last_run: crate::backend::LastRun::None,
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
                last_run: crate::backend::LastRun::None,
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
        // The popup times its messages out, which needs a root.
        let _root = floem::headless::TestRoot::new();
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
                moved_to: None,
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
                moved_to: None,
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
    fn a_head_github_moved_to_is_kept_until_the_item_opens_again() {
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");

        app.apply_with_review(
            Event::PullRequestOverviewFailed {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                head_sha: "head".into(),
                status_generation: 1,
                message: "could not refresh".into(),
                moved_to: None,
            },
            review,
        );
        assert_eq!(review.head_moved.get(), None);

        app.apply_with_review(
            Event::PullRequestOverviewFailed {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                head_sha: "head".into(),
                status_generation: 2,
                message: "moved".into(),
                moved_to: Some("newer".into()),
            },
            review,
        );
        assert_eq!(review.head_moved.get().as_deref(), Some("newer"));

        review.begin("Reading the pull request");
        assert_eq!(review.head_moved.get(), None);
    }

    #[test]
    fn a_refreshed_pull_request_on_another_head_marks_it_moved() {
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        let item = review.item.get().unwrap();

        app.apply_with_review(
            Event::PullRequestUpdated {
                item_id: "wt".into(),
                item: item.clone(),
                pull_request: Arc::new(pull_request(7, "head", "open")),
                merge_attempt_id: None,
            },
            review,
        );
        assert_eq!(review.head_moved.get(), None);

        app.apply_with_review(
            Event::PullRequestUpdated {
                item_id: "wt".into(),
                item,
                pull_request: Arc::new(pull_request(7, "newer", "open")),
                merge_attempt_id: None,
            },
            review,
        );
        assert_eq!(review.head_moved.get().as_deref(), Some("newer"));
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
                viewer_can_delete: false,
                viewer_can_update: false,
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
                reactors: vec![],
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

    /// One of GitHub's threads on the item, `resolved` or not, which the
    /// reader may resolve.
    fn github_thread(resolved: bool) -> perch_github::CommentThread {
        perch_github::CommentThread {
            id: "gh-31".into(),
            worktree_id: "wt".into(),
            origin: perch_github::ThreadOrigin::GitHub,
            github_id: Some(31),
            anchor: None,
            outdated: false,
            resolved,
            node_id: Some("PRRT_31".into()),
            can_resolve: true,
            created_at: "2026-09-19T12:00:00Z".parse().unwrap(),
            messages: Vec::new(),
        }
    }

    #[test]
    fn a_resolve_shows_before_github_answers_and_the_threads_say_after() {
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        let threads = |resolved: bool| Event::CommentThreads {
            item_id: "wt".into(),
            threads: vec![github_thread(resolved)],
            review_group_id: None,
            review_request_id: None,
        };
        app.apply_with_review(threads(false), review);
        assert!(!review.is_resolved("gh-31"));

        // Shown resolved before GitHub is asked, and asked once.
        assert!(review.begin_resolve("gh-31", true));
        assert!(review.is_resolved("gh-31"));
        assert!(review.is_resolving("gh-31"));
        assert!(!review.begin_resolve("gh-31", false), "the first ask is unanswered");
        // A thread list read before the answer does not undo it.
        app.apply_with_review(threads(false), review);
        assert!(review.is_resolved("gh-31"));

        // GitHub took it: the threads come first, then the settling.
        app.apply_with_review(threads(true), review);
        let settled = |item: &str| Event::ThreadResolveSettled {
            item_id: item.into(),
            thread_id: "gh-31".into(),
        };
        app.apply_with_review(settled("other"), review);
        assert!(review.is_resolving("gh-31"), "another item's answer is not this one's");
        app.apply_with_review(settled("wt"), review);
        assert!(!review.is_resolving("gh-31"));
        assert!(review.is_resolved("gh-31"));

        // Refused: the threads never changed, and the thread is back as
        // they have it.
        assert!(review.begin_resolve("gh-31", false));
        assert!(!review.is_resolved("gh-31"));
        app.apply_with_review(settled("wt"), review);
        assert!(review.is_resolved("gh-31"));

        // Leaving the item forgets an ask in flight.
        assert!(review.begin_resolve("gh-31", false));
        review.close();
        assert!(!review.is_resolving("gh-31"));
    }

    /// Whether the issue may be closed, and the closed issue, land on the
    /// view showing that issue and no other.
    #[test]
    fn an_issues_closability_and_its_closed_self_land_only_on_that_issue() {
        let app = AppState::new();
        let review = review_of("wt");
        let issue = |state: &str| perch_github::Issue {
            number: 42,
            node_id: "I_42".into(),
            title: "Fence the manifest".into(),
            body: String::new(),
            reactions: Default::default(),
            state: state.into(),
            state_reason: None,
            author_login: "a".into(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            comments: 0,
            created_at: None,
            updated_at: "2026-09-18T10:00:00Z".parse().unwrap(),
            repo_full_name: "slatedb/slatedb".into(),
        };
        review.issue.set(Some(Arc::new(issue("open"))));
        let closable = |item_id: &str, number: u64| Event::IssueClosable {
            item_id: item_id.into(),
            repo_full_name: "slatedb/slatedb".into(),
            number,
            closable: true,
        };
        app.apply_with_review(closable("other", 42), review);
        app.apply_with_review(closable("wt", 43), review);
        assert_eq!(review.issue_closable.get(), None);
        app.apply_with_review(closable("wt", 42), review);
        assert_eq!(review.issue_closable.get(), Some(true));

        let item = review.item.get_untracked().unwrap();
        app.apply_with_review(
            Event::IssueUpdated {
                item_id: "wt".into(),
                item,
                issue: Arc::new(issue("closed")),
            },
            review,
        );
        assert!(!review.issue.get().unwrap().is_open());
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
                events: Arc::new(Vec::new()),
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
                events: Arc::new(Vec::new()),
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
        review.review_open.set(true);
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
                review: Some(Arc::new(accepted.clone())),
                posted: 0,
                unmatched: Vec::new(),
                failed: Vec::new(),
                outside_diff: Vec::new(),
            },
            review,
        );
        assert_eq!(review.submit_review_pending.get(), Some(10));
        assert!(review.review_open.get());
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
                review: Some(Arc::new(accepted.clone())),
                posted: 0,
                unmatched: Vec::new(),
                failed: Vec::new(),
                outside_diff: Vec::new(),
            },
            review,
        );
        assert!(review.submit_review_pending.with(Option::is_none));
        assert!(!review.review_open.get());
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
    fn a_review_whose_drafts_did_not_all_post_closes_the_form_and_warns() {
        // The popup times its messages out, which needs a root.
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        review.review_open.set(true);
        review.submit_review_draft.set("Some thoughts".into());
        review.submit_review_event.set(PullReviewEvent::Approve);
        review.begin_submit_review(5);

        let accepted = Review {
            node_id: String::new(),
            reactions: Default::default(),
            id: 55,
            user: "reviewer".into(),
            body: "Some thoughts".into(),
            state: "APPROVED".into(),
            submitted_at: Some("2026-09-20T18:00:00Z".parse().unwrap()),
            html_url: String::new(),
        };
        app.apply_with_review(
            Event::PullRequestReviewSubmitted {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 5,
                review: Some(Arc::new(accepted.clone())),
                posted: 1,
                unmatched: vec!["d2".into()],
                failed: vec![("d3".into(), "GitHub returned 422: no".into())],
                outside_diff: vec!["d4".into()],
            },
            review,
        );
        assert!(review.submit_review_pending.with(Option::is_none));
        assert!(!review.review_open.get(), "the review is up");
        assert!(review.submit_review_draft.get().is_empty(), "the summary posted");
        assert_eq!(review.submit_review_event.get(), PullReviewEvent::Comment);
        assert_eq!(review.submit_review_error.get(), None);
        assert_eq!(review.submit_review_outcome.get(), None);
        assert_eq!(
            app.notice().as_deref(),
            Some(
                "Posted the review and 1 comment. GitHub takes comments only on files the pull \
                 request changes, so 1 comment is still a draft. 1 comment did not \
                 post: GitHub returned 422: no. 1 comment could not be matched to what GitHub \
                 returned and is still a draft."
            )
        );
        assert!(app.toasts.with(|toasts| toasts.iter().any(|toast| matches!(
            &toast.kind,
            ToastKind::Message { tone: Tone::Warning, .. }
        ))));
        assert_eq!(*review.submitted_reviews.get(), vec![accepted]);
    }

    #[test]
    fn a_submission_with_no_review_settles_the_form_and_adds_no_review() {
        // The popup times its messages out, which needs a root.
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        review
            .discussion
            .set(Some(Arc::new(perch_github::Discussion::default())));
        review.review_open.set(true);
        review.begin_submit_review(6);

        app.apply_with_review(
            Event::PullRequestReviewSubmitted {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 6,
                review: None,
                posted: 1,
                unmatched: Vec::new(),
                failed: Vec::new(),
                outside_diff: Vec::new(),
            },
            review,
        );
        assert!(review.submit_review_pending.with(Option::is_none));
        assert!(!review.review_open.get(), "the reply posted; nothing to say");
        assert!(review.submitted_reviews.get().is_empty());
        assert!(review.discussion.get().unwrap().reviews.is_empty());

        // The form that sent it is gone: what did not post goes to the
        // popup instead.
        review.begin_submit_review(7);
        review.submit_review_pending.set(None);
        app.apply_with_review(
            Event::PullRequestReviewSubmitted {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 7,
                review: None,
                posted: 0,
                unmatched: Vec::new(),
                failed: vec![("r1".into(), "offline".into())],
                outside_diff: Vec::new(),
            },
            review,
        );
        assert_eq!(review.submit_review_outcome.get(), None);
        assert_eq!(app.notice().as_deref(), Some("1 comment did not post: offline."));
    }

    /// The answer to an issue comment lands on the composer that sent it:
    /// a refusal leaves the text with the reason, a comment that posted
    /// closes the composer. Another item's answer, and one to a send that
    /// is not the one in flight, are not this composer's.
    #[test]
    fn an_issue_comments_answer_settles_only_the_send_in_flight() {
        use crate::review::model::Destination;
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        review.open_composer(None, None, "Looks good".into(), choice);
        review.set_destination(Destination::GitHub);
        review.start_posting(3, "Looks good".into());
        let posted = |item_id: &str, token: u64, result: Result<(), String>| {
            Event::IssueCommentPosted {
                item_id: item_id.into(),
                token,
                result: result.map(|()| {
                    Arc::new(perch_github::IssueComment {
                        id: 910,
                        node_id: "IC_910".into(),
                        user: "reader".into(),
                        user_avatar_url: String::new(),
                        body: "Looks good".into(),
                        created_at: "2026-09-21T18:00:00Z".parse().unwrap(),
                        updated_at: None,
                        html_url: String::new(),
                        reactions: Default::default(),
                        viewer_can_delete: false,
                        viewer_can_update: false,
                    })
                }),
            }
        };

        app.apply_with_review(posted("other", 3, Err("no".into())), review);
        assert!(review.posting.get().is_some(), "another item's answer");
        app.apply_with_review(posted("wt", 2, Err("no".into())), review);
        assert!(review.posting.get().is_some(), "an older send's answer");

        app.apply_with_review(posted("wt", 3, Err("GitHub said no".into())), review);
        assert!(review.posting.get().is_none());
        assert!(review.composer.get().is_some(), "the text stays");
        assert_eq!(review.composer_draft.get(), "Looks good");
        assert_eq!(review.composer_error.get().as_deref(), Some("GitHub said no"));

        review.start_posting(4, "Looks good".into());
        assert!(review.composer_error.get().is_none());
        app.apply_with_review(posted("wt", 4, Ok(())), review);
        assert!(review.posting.get().is_none());
        assert!(review.composer.get().is_none(), "posted, the composer closes");
    }

    #[test]
    fn review_submission_failure_keeps_the_draft_and_selected_outcome() {
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        review.review_open.set(true);
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
        assert!(review.review_open.get());
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
        review.review_open.set(true);
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
        review.review_open.set(true);
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
        assert!(review.review_open.get());
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
                review: Some(Arc::new(Review {
                    node_id: String::new(),
                    reactions: Default::default(),
                    id: 101,
                    user: "reviewer".into(),
                    body: "Approved before navigation".into(),
                    state: "APPROVED".into(),
                    submitted_at: Some("2026-09-20T18:05:00Z".parse().unwrap()),
                    html_url:
                        "https://github.com/slatedb/slatedb/pull/7#pullrequestreview-101".into(),
                })),
                posted: 0,
                unmatched: Vec::new(),
                failed: Vec::new(),
                outside_diff: Vec::new(),
            },
            review,
        );

        assert!(review.review_open.get());
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
        assert_eq!(review.tab.get(), crate::review::model::ItemTab::Code);
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
        // The popup times its messages out, which needs a root.
        let _root = floem::headless::TestRoot::new();
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
        assert_eq!(app.notice().as_deref(), Some("retained source missing"));
    }

    #[test]
    fn an_older_historical_load_cannot_replace_threads_or_files_before_the_latest_fails() {
        // The popup times its messages out, which needs a root.
        let _root = floem::headless::TestRoot::new();
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
            github_id: None,
            anchor: None,
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
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
        assert_eq!(app.notice().as_deref(), Some("group B failed"));
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
    fn a_notice_reaches_the_popup_and_gives_up_a_sent_questions_rows() {
        // The popup times its messages out, which needs a root.
        let _root = floem::headless::TestRoot::new();
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
            app.notice().as_deref(),
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

        let first = app.current_toast().unwrap();
        assert_eq!(first.kind, ToastKind::AgentRun("first".into()));
        app.dismiss_toast(first.token);
        assert_eq!(
            app.current_toast().map(|toast| toast.kind),
            Some(ToastKind::AgentRun("second".into()))
        );
    }

    #[test]
    fn a_message_replaces_the_one_before_and_an_empty_one_takes_it_down() {
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        app.show_agent_completion("run".into());
        app.apply(Event::Notice("first".into()));
        app.apply(Event::Notice("second".into()));
        assert_eq!(app.toasts.with(VecDeque::len), 2, "the run and one message");
        assert_eq!(app.notice().as_deref(), Some("second"));

        app.apply(Event::Notice(String::new()));
        assert_eq!(app.notice(), None);
        assert_eq!(
            app.current_toast().map(|toast| toast.kind),
            Some(ToastKind::AgentRun("run".into()))
        );
    }

    /// A merge that went through is news, not a complaint.
    #[test]
    fn a_done_line_is_a_success_not_an_error() {
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        app.apply(Event::Done("Merged a/b#1 (squash)".into()));
        assert_eq!(
            app.current_toast().map(|toast| toast.kind),
            Some(ToastKind::Message {
                text: "Merged a/b#1 (squash)".into(),
                tone: Tone::Success,
                open: None,
            })
        );
    }

    #[test]
    fn a_poll_error_pops_up_once_and_not_again_while_it_lasts() {
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        let error = Status::Error {
            message: "GitHub is down".into(),
        };
        app.apply(Event::Status(error.clone()));
        assert_eq!(app.notice().as_deref(), Some("GitHub is down"));
        app.dismiss_toast(app.current_toast().unwrap().token);
        app.apply(Event::Status(Status::Refreshing));
        app.apply(Event::Status(error));
        assert_eq!(app.notice(), None, "the same error, still going");
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
                note: false,
                pending: false,
                user: false,
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
