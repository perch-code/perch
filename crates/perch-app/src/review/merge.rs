//! Pull-request checks and the deliberate, native merge box.
//!
//! The box is the popover under the bar's Merge (`merge_button`); the
//! checks are the summary's checks chip and its menu, which read their
//! words and glyphs from here. The overview is a live GitHub snapshot for
//! one exact head. Cached checks are useful to read while it arrives, but
//! every merge decision below uses the live snapshot and rechecks it again
//! when the confirmation is pressed.

use std::rc::Rc;
use std::sync::Arc;

use floem::event::EventPropagation;
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack, TextInput};
use floem::AnyView;
use perch_github::{
    Check, CheckRunState, CheckState, Checks, CommitMessage, Item, MergeMethod, MergeMethods,
    MergeState, MergeableState, PullRequest, PullRequestOverview, RepositoryPermission,
    ReviewDecision, SubjectState,
};

use crate::backend::Backend;
use crate::icons::{state_color, SidebarIcon};
use crate::review::model::{AgentChoice, ReviewState};
use crate::review::timeline::short_sha;
use crate::state::{AppState, Checked};
use crate::theme;
use crate::ui::menu::{Menu, MenuExt};
use crate::ui::widgets::{
    action_button, checkbox, clickable, editor_box, error_line, open_in_browser, prose_editor,
    sidebar_glyph, state_glyph,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct MergeConfirmation {
    item_id: String,
    repo_full_name: String,
    number: u64,
    head_sha: String,
    method: MergeMethod,
    bypass_requirements: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct MergeSnapshot {
    pull: Option<Arc<PullRequest>>,
    item: Option<Arc<Item>>,
    overview: Option<Arc<PullRequestOverview>>,
    overview_loading: bool,
    overview_error: Option<String>,
    merge_pending: bool,
    merge_error: Option<String>,
    offline: bool,
    historical: bool,
    stale: bool,
    selected_method: MergeMethod,
    bypass_requirements: bool,
    confirmation: Option<MergeConfirmation>,
    /// Whether Push can bring the pull request to the worktree, which is
    /// what keeps Merge from a worktree with unpushed commits.
    can_push: bool,
    push_pending: bool,
    push_error: Option<String>,
    /// Whether Update branch can bring the worktree to the pull request,
    /// which is what keeps Merge from a worktree behind it.
    can_update: bool,
    update_pending: bool,
    update_error: Option<String>,
    /// Whether Rebase can bring the two together when each has commits
    /// the other lacks.
    can_rebase: bool,
    rebase_pending: bool,
    /// Whether the reader may mark a draft ready for review, and a change
    /// to or from a draft under way.
    may_set_draft: bool,
    draft_pending: Option<bool>,
    /// The agent that would resolve conflicts, whether one is at it, and
    /// which way Resolve conflicts goes.
    resolver: Option<perch_agent::Backend>,
    resolving: bool,
    conflict_route: ConflictRoute,
}

/// The commit message being confirmed for a squash or a merge commit.
/// It starts as GitHub's own and outlives the confirmation, which a
/// status refresh puts away, so what was typed is there when it opens
/// again for the same head and method. Left as GitHub's, it follows
/// GitHub's, which an edit of the description changes.
#[derive(Clone, Copy)]
struct MessageDraft {
    title: RwSignal<String>,
    body: RwSignal<String>,
    /// The item, head and method the text was filled for.
    filled_for: RwSignal<Option<(String, String, MergeMethod)>>,
    /// GitHub's message the text was filled with.
    filled_with: RwSignal<CommitMessage>,
}

impl MessageDraft {
    fn new() -> Self {
        Self {
            title: RwSignal::new(String::new()),
            body: RwSignal::new(String::new()),
            filled_for: RwSignal::new(None),
            filled_with: RwSignal::new(CommitMessage::default()),
        }
    }

    /// Put GitHub's message in, unless the draft is already this
    /// request's: then only GitHub's message changed since, over text
    /// the reader hasn't touched.
    fn fill(self, request: &MergeConfirmation, default: Option<&CommitMessage>) {
        let key = (
            request.item_id.clone(),
            request.head_sha.clone(),
            request.method,
        );
        let default = default.cloned().unwrap_or_default();
        let ours = self
            .filled_for
            .with_untracked(|filled| filled.as_ref() == Some(&key));
        let stale = self.filled_with.with_untracked(|filled| {
            *filled != default
                && self.title.with_untracked(|title| *title == filled.title)
                && self.body.with_untracked(|body| *body == filled.body)
        });
        if ours && !stale {
            return;
        }
        self.title.set(default.title.clone());
        self.body.set(default.body.clone());
        self.filled_for.set(Some(key));
        self.filled_with.set(default);
    }

    fn message(self) -> CommitMessage {
        CommitMessage {
            title: self.title.get_untracked().trim().to_string(),
            body: self.body.get_untracked(),
        }
    }
}

/// Whether GitHub takes a commit message for `method`: a rebase keeps
/// each commit's own.
fn takes_message(method: MergeMethod) -> bool {
    method != MergeMethod::Rebase
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisabledReason {
    Offline,
    Historical,
    StaleCheckout,
    Closed,
    Draft,
    Unknown,
    NoWritePermission,
    NoMergeMethod,
    Requirements,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeDecision {
    Loading,
    Disabled(DisabledReason),
    Conflicts,
    Ready,
    Bypass,
}

#[derive(Debug, Clone, Copy)]
struct MergeFacts {
    offline: bool,
    historical: bool,
    stale: bool,
    /// A worktree of the reader's own is never moved to the pull
    /// request's head, so its files can be ahead of, behind, or beside
    /// what GitHub would merge.
    diverged: bool,
    open: bool,
    draft: bool,
    overview_present: bool,
    overview_loading: bool,
    heads_match: bool,
    /// The checkout holds GitHub's head and commits of its own on top,
    /// none missing: conflicts merged here go up with them.
    only_ahead: bool,
    has_write_permission: bool,
    mergeable: Option<MergeableState>,
    merge_state: Option<MergeState>,
    can_bypass: bool,
    has_merge_method: bool,
}

/// Which way Resolve conflicts goes. With an agent, where one may work
/// here, unless the reader picks GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConflictRoute {
    Agent,
    GitHub,
}

/// What the merge box needs to offer an agent for conflicts.
#[derive(Clone, Copy)]
struct Resolver {
    /// The agent that would resolve them, the one last asked here or
    /// config's, or None where the reader couldn't push what it made:
    /// someone else's pull request they can't push to, or one with no
    /// checkout.
    agent: Memo<Option<AgentChoice>>,
    /// An agent asked, and not done yet.
    busy: Memo<bool>,
    route: RwSignal<ConflictRoute>,
}

/// The first line of what an agent is asked when it resolves conflicts,
/// by which its run is known again after the item is reopened.
pub(crate) const CONFLICTS_REQUEST: &str = "Resolve the merge conflicts with ";

/// What an agent is asked to resolve conflicts with `base`, which the
/// checkout has at `rev`, on `branch`: a merge, since the pull request's
/// commits are on GitHub already and a rebase would need a force push.
pub(crate) fn conflicts_request(base: &str, rev: &str, branch: &str) -> String {
    format!(
        "{CONFLICTS_REQUEST}{base}.\n\n\
         Merge {rev}, which is {base} as GitHub has it now, into {branch} with \
         `git merge -m \"Merge {base} into {branch}\" {rev}`. The merge writes to the git \
         directory as a commit does, so run it the way you would run a commit here. Resolve \
         each conflict so the changes on both sides still hold, then build and run the tests \
         that cover what you touched. Commit the merge once it is clean. Do not rebase, and \
         do not push."
    )
}

/// The merge box's own state: the method picked, the bypass consented
/// to, the confirmation under way and its commit message, and the agent
/// that would resolve conflicts. Made once with the bar's Merge, whose
/// label and method menu read it, and shared with the popover that
/// button opens, which draws `section`.
#[derive(Clone, Copy)]
pub(crate) struct MergeBox {
    selected_method: RwSignal<MergeMethod>,
    bypass_requirements: RwSignal<bool>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
    message: MessageDraft,
    can_push: Memo<bool>,
    may_set_draft: Memo<bool>,
    resolver: Resolver,
}

/// What the bar's Merge shows for the open item, when it shows at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BarMerge {
    /// A draft the reader may mark ready for review: the button does that,
    /// and says so while it goes (`pending`).
    ReadyForReview { pending: bool },
    /// Merge, named for the method picked. `ready` when GitHub would take
    /// it now, which makes the button green; quiet otherwise, and the
    /// popover says why. `pick` when the repository allows more than one
    /// method, for the chevron beside it.
    Merge {
        label: String,
        ready: bool,
        pick: bool,
    },
}

impl MergeBox {
    pub(crate) fn new(app: AppState, state: ReviewState) -> Self {
        let merge = MergeBox {
            // GitHub's common default, and the least surprising default
            // when it is enabled. `effective_method` falls back to another
            // repository-enabled method when squash is unavailable.
            selected_method: RwSignal::new(MergeMethod::Squash),
            bypass_requirements: RwSignal::new(false),
            confirmation: RwSignal::new(None),
            message: MessageDraft::new(),
            can_push: Memo::new(move |_| crate::review::summary::can_push(app, state)),
            may_set_draft: Memo::new(move |_| crate::review::summary::may_set_draft(app, state)),
            resolver: Resolver {
                agent: Memo::new(move |_| conflicts_agent(app, state)),
                busy: Memo::new(move |_| resolving_conflicts(app, state)),
                route: RwSignal::new(ConflictRoute::Agent),
            },
        };

        // Consent and confirmation apply to one exact head, never to
        // whichever pull request happens to occupy this long-lived review
        // view next.
        Effect::new(move |previous: Option<(Option<String>, Option<String>)>| {
            let identity = (
                state
                    .item
                    .with(|item| item.as_ref().map(|item| item.id.clone())),
                state
                    .pull_request
                    .with(|pull| pull.as_ref().map(|pull| pull.head_sha.clone())),
            );
            if previous.as_ref() != Some(&identity) {
                merge.selected_method.set(MergeMethod::Squash);
                merge.bypass_requirements.set(false);
                merge.confirmation.set(None);
                merge.resolver.route.set(ConflictRoute::Agent);
            }
            identity
        });
        // A same-head refresh can represent different checks, protection
        // rules, or permissions. Bypass consent is therefore one-shot for
        // the live status generation that presented it, not a sticky
        // preference.
        Effect::new(move |previous: Option<u64>| {
            let generation = state.pull_request_status_generation.get();
            if previous != Some(generation) {
                merge.bypass_requirements.set(false);
                merge.confirmation.set(None);
            }
            generation
        });
        merge
    }

    /// The box: how the merge stands and why, the errors it ended in, and
    /// what can be done, from the merge and its confirmation to Push,
    /// Update branch and resolving conflicts.
    pub(crate) fn section(self, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
        merge_section(
            state,
            backend,
            self.selected_method,
            self.bypass_requirements,
            self.confirmation,
            self.message,
            self.can_push,
            self.may_set_draft,
            self.resolver,
        )
    }

    /// Put away a confirmation under way, as shutting the popover does.
    pub(crate) fn cancel(self) {
        self.confirmation.set(None);
    }

    /// What the bar's Merge shows: Ready for review on a draft the reader
    /// may mark ready, else Merge for a reader who may merge an open pull
    /// request, green only when GitHub would take it as it stands. A
    /// branch behind its base is quiet too, though GitHub may take it.
    /// Nothing on an issue, a branch, a pull request merged or closed,
    /// offline, or on an old review's snapshot. Tracked.
    pub(crate) fn bar(self, state: ReviewState) -> Option<BarMerge> {
        if state.offline.get() || state.historical_review.with(Option::is_some) {
            return None;
        }
        let pull = state.pull_request.get()?;
        if !pull.is_open() || pull.merged {
            return None;
        }
        let draft_pending = state.draft_pending.get();
        if pull.draft && (self.may_set_draft.get() || draft_pending.is_some()) {
            return Some(BarMerge::ReadyForReview {
                pending: draft_pending == Some(false),
            });
        }
        if !may_merge(state) {
            return None;
        }
        let item = state.item.get()?;
        let overview = state.pull_request_overview.get();
        let decision = decide_merge(merge_facts(
            &pull,
            &item,
            overview.as_deref(),
            false,
            false,
            state.stale_head.with(Option::is_some),
            state.pull_request_overview_loading.get(),
        ));
        let methods = overview
            .as_deref()
            .map(|overview| overview.allowed_merge_methods);
        let pending = state.merge_pending.get();
        let label = if pending {
            "Merging…"
        } else {
            methods
                .and_then(|methods| effective_method(self.selected_method.get(), methods))
                .map_or("Merge", method_label)
        };
        let behind = overview
            .as_deref()
            .is_some_and(|overview| overview.merge_state == MergeState::Behind);
        Some(BarMerge::Merge {
            label: label.to_string(),
            ready: decision == MergeDecision::Ready && !behind && !pending,
            pick: !pending && methods.is_some_and(|methods| method_count(methods) > 1),
        })
    }

    /// The methods the repository allows, the one picked checked. Picking
    /// another puts a confirmation under way away, as it was for the
    /// method picked before.
    pub(crate) fn method_menu(self, state: ReviewState) -> Menu {
        match state.pull_request_overview.with_untracked(|overview| {
            overview
                .as_deref()
                .map(|overview| overview.allowed_merge_methods)
        }) {
            Some(methods) => merge_method_menu(methods, self.selected_method, self.confirmation),
            None => Menu::new(),
        }
    }
}

/// How many merge methods the repository allows.
fn method_count(methods: MergeMethods) -> usize {
    [methods.merge, methods.squash, methods.rebase]
        .into_iter()
        .filter(|allowed| *allowed)
        .count()
}

/// Whether the reader may merge the open pull request, as the merge box
/// itself decides who may: open and not merged, GitHub reached and not an
/// old review's snapshot, and the live overview saying they may write or
/// merge as an admin. The bar's Merge is offered on this; what else
/// stands in the way, checks or conflicts or a worktree behind, is the
/// box's to say once it is open. Tracked.
pub(crate) fn may_merge(state: ReviewState) -> bool {
    if state.offline.get() || state.historical_review.with(Option::is_some) {
        return false;
    }
    let open = state.pull_request.with(|pull| {
        pull.as_ref()
            .is_some_and(|pull| pull.is_open() && !pull.merged)
    });
    open && state
        .pull_request_overview
        .with(|overview| overview.as_deref().is_some_and(writes))
}

/// Whether the overview says the reader may merge: write access, or an
/// admin's leave to merge past the rules.
fn writes(overview: &PullRequestOverview) -> bool {
    overview.viewer_can_merge_as_admin
        || matches!(
            overview.viewer_permission,
            Some(
                RepositoryPermission::Admin
                    | RepositoryPermission::Maintain
                    | RepositoryPermission::Write
            )
        )
}

/// The checks for the head: the live overview's when it has come, else
/// the cached rollup. Tracked.
pub(crate) fn current_checks(state: ReviewState) -> Option<Arc<Checks>> {
    state
        .pull_request_overview
        .with(|overview| {
            overview
                .as_ref()
                .map(|overview| Arc::new(overview.checks.clone()))
        })
        .or_else(|| state.checks.get())
}

fn check_name(check: &Check) -> String {
    let mut name = match check.workflow.as_deref() {
        Some(workflow) if workflow != check.name => format!("{workflow} / {}", check.name),
        _ => check.name.clone(),
    };
    if let Some(event) = check.event.as_deref().filter(|event| !event.is_empty()) {
        name.push_str(" (");
        name.push_str(event);
        name.push(')');
    }
    name
}

/// A check's name, with its workflow and event, and what it says of
/// itself.
pub(crate) fn check_label(check: &Check) -> String {
    let name = check_name(check);
    match check
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        Some(description) => format!("{name} — {description}"),
        None => name,
    }
}

/// How a check stands, and how long it took once it is done.
pub(crate) fn check_status_label(check: &Check) -> String {
    let status = check_status(check.state);
    let Some(seconds) = check
        .started_at
        .zip(check.completed_at)
        .map(|(started, completed)| completed.as_second().saturating_sub(started.as_second()))
        .filter(|seconds| *seconds >= 0)
    else {
        return status.to_string();
    };
    format!("{status} in {}", compact_duration(seconds))
}

fn compact_duration(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else {
        let hours = seconds / 3_600;
        let minutes = (seconds % 3_600) / 60;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        }
    }
}

/// A check's glyph, in the colour of how it stands.
pub(crate) fn check_icon(state: CheckRunState) -> (SidebarIcon, fn() -> Color) {
    if state.is_passing() {
        (SidebarIcon::Check, theme::open)
    } else if state.is_failing() {
        (SidebarIcon::Close, theme::closed)
    } else if state == CheckRunState::Waiting {
        (SidebarIcon::Waiting, theme::fg_subtle)
    } else if state.is_pending() {
        (SidebarIcon::Refresh, theme::draft_yellow)
    } else {
        (SidebarIcon::Warning, theme::fg_subtle)
    }
}

fn check_status(state: CheckRunState) -> &'static str {
    match state {
        CheckRunState::Pending => "In progress",
        CheckRunState::Waiting => "Waiting",
        CheckRunState::Success => "Passed",
        CheckRunState::Failure => "Failed",
        CheckRunState::Neutral => "Neutral",
        CheckRunState::Skipped => "Skipped",
        CheckRunState::Cancelled => "Cancelled",
        CheckRunState::TimedOut => "Timed out",
        CheckRunState::ActionRequired => "Action required",
        CheckRunState::StartupFailure => "Startup failure",
        CheckRunState::Stale => "Stale",
        CheckRunState::Unknown => "Unknown",
    }
}

#[allow(clippy::too_many_arguments)]
fn merge_section(
    state: ReviewState,
    backend: Rc<Backend>,
    selected_method: RwSignal<MergeMethod>,
    bypass_requirements: RwSignal<bool>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
    message: MessageDraft,
    can_push: Memo<bool>,
    may_set_draft: Memo<bool>,
    resolver: Resolver,
) -> impl IntoView {
    dyn_container(
        move || MergeSnapshot {
            pull: state.pull_request.get(),
            item: state.item.get(),
            overview: state.pull_request_overview.get(),
            overview_loading: state.pull_request_overview_loading.get(),
            overview_error: state.pull_request_overview_error.get(),
            merge_pending: state.merge_pending.get(),
            merge_error: state.merge_error.get(),
            offline: state.offline.get(),
            historical: state.historical_review.with(Option::is_some),
            stale: state.stale_head.with(Option::is_some),
            selected_method: selected_method.get(),
            bypass_requirements: bypass_requirements.get(),
            confirmation: confirmation.get(),
            can_push: can_push.get(),
            push_pending: state.push_pending.get(),
            push_error: state.push_error.get(),
            can_update: crate::review::summary::can_update(state),
            update_pending: state.update_pending.get(),
            update_error: state.update_error.get(),
            can_rebase: crate::review::summary::can_rebase(state),
            rebase_pending: state.rebase_pending.get(),
            may_set_draft: may_set_draft.get(),
            draft_pending: state.draft_pending.get(),
            resolver: resolver
                .agent
                .with(|agent| agent.as_ref().map(|(backend, ..)| *backend)),
            resolving: resolver.busy.get(),
            conflict_route: resolver.route.get(),
        },
        move |snapshot| {
            merge_content(
                state,
                backend.clone(),
                bypass_requirements,
                confirmation,
                message,
                resolver,
                snapshot,
            )
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn merge_content(
    state: ReviewState,
    backend: Rc<Backend>,
    bypass_requirements: RwSignal<bool>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
    message: MessageDraft,
    resolver: Resolver,
    snapshot: MergeSnapshot,
) -> AnyView {
    let (Some(pull), Some(item)) = (snapshot.pull.as_ref(), snapshot.item.as_ref()) else {
        return Empty::new().into_any();
    };
    // Nothing is left to decide once it is merged or closed: it says so,
    // with no button that can never be pressed.
    if let Some((state, title, detail)) = finished_copy(pull) {
        return status_row(
            state_glyph(state, move || state_color(state)),
            title,
            detail,
        )
        .debug_name("merge-finished")
        .into_any();
    }
    let decision = merge_decision(&snapshot, pull, item);
    let status = merge_status(
        decision,
        snapshot.overview.as_deref(),
        item.divergence(),
        conflicts_detail(&snapshot, &pull.base_ref),
    );
    let controls = merge_controls(
        state,
        backend,
        bypass_requirements,
        confirmation,
        message,
        resolver,
        decision,
        snapshot.clone(),
        pull.clone(),
        item.clone(),
    );

    Stack::vertical((
        status,
        error_line(snapshot.overview_error.clone(), "merge-overview-error"),
        error_line(snapshot.merge_error.clone(), "merge-error"),
        error_line(snapshot.push_error.clone(), "merge-push-error"),
        error_line(snapshot.update_error.clone(), "merge-update-error"),
        controls,
    ))
    .style(|s| s.flex_col().width_full().gap(12.0))
    .into_any()
}

fn merge_status(
    decision: MergeDecision,
    overview: Option<&PullRequestOverview>,
    diverged: Option<perch_github::Diverged>,
    conflicts: String,
) -> impl IntoView {
    let (icon, color, title, detail): (_, fn() -> Color, _, _) = match decision {
        MergeDecision::Loading => (
            SidebarIcon::Refresh,
            theme::draft_yellow,
            "Checking merge requirements".to_string(),
            "Fetching a fresh status from GitHub.".to_string(),
        ),
        MergeDecision::Conflicts => (
            SidebarIcon::Warning,
            theme::draft,
            "This branch has conflicts that must be resolved".to_string(),
            conflicts,
        ),
        MergeDecision::Ready => (
            SidebarIcon::Check,
            theme::open,
            "Ready to merge".to_string(),
            ready_detail(overview),
        ),
        MergeDecision::Bypass => (
            SidebarIcon::Warning,
            theme::closed,
            "Merging is blocked".to_string(),
            blocker_detail(overview),
        ),
        MergeDecision::Disabled(reason) => {
            let (title, detail) = disabled_copy(reason, overview, diverged);
            (SidebarIcon::Warning, theme::fg_subtle, title, detail)
        }
    };

    status_row(sidebar_glyph(icon, color), title, detail)
}

/// What a merged or closed pull request's box says, or None while it is
/// open.
fn finished_copy(pull: &PullRequest) -> Option<(SubjectState, String, String)> {
    if pull.merged {
        let detail = match pull.merged_by.as_deref().filter(|by| !by.is_empty()) {
            Some(by) => format!("{by} merged this into {}.", pull.base_ref),
            None => format!("This was merged into {}.", pull.base_ref),
        };
        return Some((SubjectState::PrMerged, "Merged".to_string(), detail));
    }
    (!pull.is_open()).then(|| {
        (
            SubjectState::PrClosed,
            "Closed".to_string(),
            "This pull request was closed without merging.".to_string(),
        )
    })
}

fn status_row(icon: impl IntoView + 'static, title: String, detail: String) -> Stack {
    Stack::horizontal((
        Container::new(icon.style(|s| s.size(16.0, 16.0))).style(|s| {
            s.size(30.0, 30.0)
                .items_center()
                .justify_center()
                .background(theme::canvas_subtle())
                .border_radius(15.0)
                .margin_right(10.0)
        }),
        Stack::vertical((
            Label::derived(move || title.clone()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg())
            }),
            Label::derived(move || detail.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .margin_top(2.0)
                    .width_full()
                    .text_wrap()
            }),
        ))
        .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0_f32)),
    ))
    .style(|s| s.width_full().items_start())
}

#[allow(clippy::too_many_arguments)]
fn merge_controls(
    state: ReviewState,
    backend: Rc<Backend>,
    bypass_requirements: RwSignal<bool>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
    message: MessageDraft,
    resolver: Resolver,
    decision: MergeDecision,
    snapshot: MergeSnapshot,
    pull: Arc<PullRequest>,
    item: Arc<Item>,
) -> AnyView {
    match decision {
        MergeDecision::Conflicts => {
            conflict_controls(state, backend, resolver, &snapshot, &pull, &item.id)
        }
        MergeDecision::Loading => {
            merge_button("Checking status…".to_string(), false, || false, || {}).into_any()
        }
        // Unpushed commits are the one thing keeping Merge, and Push is
        // what clears them; the pull request is fetched again after.
        MergeDecision::Disabled(DisabledReason::StaleCheckout)
            if snapshot.can_push || snapshot.push_pending =>
        {
            let label = if snapshot.push_pending {
                "Pushing…".to_string()
            } else {
                push_label(item.divergence().and_then(|d| d.ahead))
            };
            let pending = snapshot.push_pending;
            merge_button(
                label,
                true,
                move || !pending,
                move || crate::review::summary::push(state, &backend),
            )
            .into_any()
        }
        // The worktree is behind the pull request, or hasn't its head, and
        // Update branch brings it there.
        MergeDecision::Disabled(DisabledReason::StaleCheckout)
            if snapshot.can_update || snapshot.update_pending =>
        {
            let pending = snapshot.update_pending;
            let label = if pending {
                "Updating…"
            } else {
                "Update branch"
            };
            merge_button(
                label.to_string(),
                true,
                move || !pending,
                move || crate::review::summary::update(state, &backend),
            )
            .into_any()
        }
        // Each has commits the other lacks, and Rebase puts the worktree's
        // on top; Push is offered after.
        MergeDecision::Disabled(DisabledReason::StaleCheckout)
            if snapshot.can_rebase || snapshot.rebase_pending =>
        {
            let pending = snapshot.rebase_pending;
            let label = if pending { "Rebasing…" } else { "Rebase" };
            merge_button(
                label.to_string(),
                true,
                move || !pending,
                move || crate::review::summary::rebase(state, &backend),
            )
            .into_any()
        }
        // A draft can't be merged, and Ready for review is what clears it.
        // Held while it goes and the pull request is read again, so the
        // merge state from before it does not show between.
        MergeDecision::Disabled(DisabledReason::Draft)
            if snapshot.draft_pending.is_some() || (pull.draft && snapshot.may_set_draft) =>
        {
            let pending = snapshot.draft_pending.is_some();
            let label = if snapshot.draft_pending == Some(false) {
                "Marking ready for review…"
            } else {
                "Ready for review"
            };
            merge_button(
                label.to_string(),
                true,
                move || !pending,
                move || crate::review::summary::set_draft(state, &backend, false),
            )
            .into_any()
        }
        MergeDecision::Disabled(_) => {
            merge_button("Merge unavailable".to_string(), false, || false, || {}).into_any()
        }
        MergeDecision::Ready | MergeDecision::Bypass => {
            let Some(overview) = snapshot.overview.as_ref() else {
                return Empty::new().into_any();
            };
            if snapshot.merge_pending {
                return merge_button("Merging…".to_string(), true, || false, || {}).into_any();
            }

            let Some(method) =
                effective_method(snapshot.selected_method, overview.allowed_merge_methods)
            else {
                return Empty::new().into_any();
            };
            let bypass_needed = decision == MergeDecision::Bypass;
            let bypass = bypass_needed && snapshot.bypass_requirements;
            let valid_confirmation = snapshot.confirmation.as_ref().filter(|candidate| {
                candidate.item_id == item.id
                    && candidate.repo_full_name == pull.repo_full_name
                    && candidate.number == pull.number
                    && candidate.head_sha == overview.head_sha
                    && candidate.method == method
                    && candidate.bypass_requirements == bypass
            });

            if let Some(request) = valid_confirmation.cloned() {
                return confirmation_controls(state, backend, confirmation, message, request, pull)
                    .into_any();
            }

            // Hidden, not just empty, so the column's gap skips it.
            let bypass_control = if bypass_needed {
                bypass_checkbox(bypass_requirements, confirmation).into_any()
            } else {
                Empty::new().style(|s| s.hide()).into_any()
            };

            let request = MergeConfirmation {
                item_id: item.id.clone(),
                repo_full_name: pull.repo_full_name.clone(),
                number: pull.number,
                head_sha: overview.head_sha.clone(),
                method,
                bypass_requirements: bypass,
            };
            let start_request = request.clone();
            let default_message = overview.commit_message(method).cloned();
            let start_enabled = !bypass_needed || bypass;
            let action_label = if bypass_needed {
                format!("Bypass rules and merge ({})", method_short(method))
            } else {
                method_label(method).to_string()
            };
            let action = merge_button(
                action_label,
                true,
                move || start_enabled,
                move || {
                    message.fill(&start_request, default_message.as_ref());
                    confirmation.set(Some(start_request.clone()));
                },
            )
            .style(|s| s.flex_grow(1.0_f32));
            // The method is picked from the chevron beside the bar's Merge,
            // which names it; the box has no picker of its own.
            Stack::vertical((bypass_control, action))
                .style(|s| s.flex_col().width_full().gap(8.0))
                .into_any()
        }
    }
}

fn bypass_checkbox(
    bypass_requirements: RwSignal<bool>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
) -> impl IntoView {
    Stack::horizontal((
        checkbox(
            move || {
                if bypass_requirements.get() {
                    Checked::Full
                } else {
                    Checked::Empty
                }
            },
            move || {
                bypass_requirements.update(|checked| *checked = !*checked);
                confirmation.set(None);
            },
        ),
        Label::derived(|| "Merge without waiting for requirements to be met".to_string()).style(
            |s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::closed())
                    .selectable(false)
            },
        ),
    ))
    .on_event_stop(el::Click, move |_, _| {
        bypass_requirements.update(|checked| *checked = !*checked);
        confirmation.set(None);
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            bypass_requirements.update(|checked| *checked = !*checked);
            confirmation.set(None);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| {
        clickable(s)
            .items_center()
            .margin_left(-5.0)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
    })
    .debug_name("merge-bypass")
}

fn confirmation_controls(
    state: ReviewState,
    backend: Rc<Backend>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
    message: MessageDraft,
    request: MergeConfirmation,
    pull: Arc<PullRequest>,
) -> impl IntoView {
    let with_message = takes_message(request.method);
    let summary = format!(
        "Merge {} into {} using {}?",
        short_sha(&request.head_sha),
        pull.base_ref,
        method_short(request.method),
    );
    let cancel = merge_button(
        "Cancel".to_string(),
        false,
        || true,
        move || confirmation.set(None),
    );

    let enabled_request = request.clone();
    let submit_request = request.clone();
    let submit = merge_button(
        if request.bypass_requirements {
            "Confirm bypass and merge".to_string()
        } else {
            "Confirm merge".to_string()
        },
        true,
        move || {
            request_is_current(state, &enabled_request)
                && !(with_message && message.title.with(|title| title.trim().is_empty()))
        },
        move || {
            if !request_is_current(state, &submit_request) {
                confirmation.set(None);
                return;
            }
            let message = with_message.then(|| message.message());
            if message
                .as_ref()
                .is_some_and(|message| message.title.is_empty())
            {
                return;
            }
            state.merge_pending.set(true);
            state.merge_error.set(None);
            confirmation.set(None);
            backend.merge_pull_request(
                &submit_request.item_id,
                &submit_request.repo_full_name,
                submit_request.number,
                &submit_request.head_sha,
                submit_request.method,
                submit_request.bypass_requirements,
                message,
            );
        },
    );

    let fields = if with_message {
        message_fields(message).into_any()
    } else {
        Empty::new().into_any()
    };

    Stack::vertical((
        Label::derived(move || summary.clone())
            .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_muted())),
        fields,
        Stack::horizontal((Empty::new().style(|s| s.flex_grow(1.0_f32)), cancel, submit))
            .style(|s| s.width_full().items_center().gap(8.0)),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .gap(10.0)
            .padding(12.0)
            .background(theme::canvas_subtle())
            .border(1.0)
            .border_color(theme::border_muted())
            .border_radius(theme::RADIUS)
    })
}

/// The commit's title and description, as GitHub's merge box has them.
fn message_fields(message: MessageDraft) -> impl IntoView {
    let title = TextInput::new(message.title)
        .placeholder("Commit title")
        .debug_name("merge-commit-title")
        .style(|s| {
            s.width_full()
                .height(30.0)
                .font_size(theme::TEXT_BODY)
                .padding_horiz(8.0)
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(theme::RADIUS)
                .focus(|s| s.border_color(theme::accent()))
        });

    let editor = prose_editor(
        message.body.get_untracked(),
        "Add an optional extended description\u{2026}",
        None,
    );
    let editor = editor
        .update(move |update| {
            if let Some(editor) = update.editor {
                message.body.set(editor.doc().text().to_string());
            }
        })
        .style(|s| {
            s.size_full()
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
        })
        .debug_name("merge-commit-body");
    let body = editor_box(
        editor,
        || (theme::COMPOSER_MIN_HEIGHT, theme::COMPOSER_GROWN_HEIGHT),
        true,
    );

    Stack::vertical((title, body)).style(|s| s.flex_col().width_full().gap(8.0))
}

/// The small button beside a merge box action that picks how it goes.
fn picker(label: impl Fn() -> String + 'static, menu: impl Fn() -> Menu + 'static) -> impl View {
    Stack::horizontal((
        Label::derived(label).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg())
                .selectable(false)
                .pointer_events_none()
                .margin_right(5.0)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::fg_subtle)
            .style(|s| s.size(9.0, 9.0).pointer_events_none()),
    ))
    .dropdown(menu)
    .style(|s| {
        clickable(s)
            .items_center()
            .height(30.0)
            .padding_horiz(9.0)
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .hover(|s| s.background(theme::canvas_hover()))
    })
    .into_view()
}

fn merge_method_menu(
    methods: MergeMethods,
    selected_method: RwSignal<MergeMethod>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
) -> Menu {
    let selected = effective_method(selected_method.get_untracked(), methods);
    let mut menu = Menu::new();

    if methods.merge {
        menu = menu.item("Create a merge commit", move |item| {
            item.checked(selected == Some(MergeMethod::Merge))
                .action(move || {
                    selected_method.set(MergeMethod::Merge);
                    confirmation.set(None);
                })
        });
    }
    if methods.squash {
        menu = menu.item("Squash and merge", move |item| {
            item.checked(selected == Some(MergeMethod::Squash))
                .action(move || {
                    selected_method.set(MergeMethod::Squash);
                    confirmation.set(None);
                })
        });
    }
    if methods.rebase {
        menu = menu.item("Rebase and merge", move |item| {
            item.checked(selected == Some(MergeMethod::Rebase))
                .action(move || {
                    selected_method.set(MergeMethod::Rebase);
                    confirmation.set(None);
                })
        });
    }
    menu
}

/// Resolve conflicts with the agent, where one may work here, or on
/// GitHub, picked beside it as the merge method is. While the agent is at
/// it, only that is shown.
fn conflict_controls(
    state: ReviewState,
    backend: Rc<Backend>,
    resolver: Resolver,
    snapshot: &MergeSnapshot,
    pull: &PullRequest,
    item_id: &str,
) -> AnyView {
    let on_github = || {
        let url = conflicts_url(&pull.html_url);
        let open_url = url.clone();
        merge_button(
            "Resolve conflicts on GitHub".to_string(),
            false,
            move || url.is_some(),
            move || {
                if let Some(url) = open_url.as_deref() {
                    open_in_browser(url);
                }
            },
        )
    };
    let Some(agent) = snapshot.resolver else {
        return on_github().into_any();
    };
    if snapshot.resolving {
        return merge_button("Resolving conflicts…".to_string(), true, || false, || {}).into_any();
    }
    let action = match snapshot.conflict_route {
        ConflictRoute::GitHub => on_github().into_any(),
        ConflictRoute::Agent => {
            let item_id = item_id.to_string();
            merge_button(
                format!("Resolve conflicts with {}", agent_name(agent)),
                true,
                || true,
                move || {
                    let Some(choice) = resolver.agent.get_untracked() else {
                        return;
                    };
                    state.resolving_conflicts.set(true);
                    backend.resolve_conflicts(&item_id, choice);
                },
            )
            .into_any()
        }
    };
    let route = resolver.route;
    let base = pull.base_ref.clone();
    Stack::horizontal((
        action.style(|s| s.flex_grow(1.0_f32)),
        picker(
            move || match route.get() {
                ConflictRoute::Agent => agent_name(agent).to_string(),
                ConflictRoute::GitHub => "GitHub".to_string(),
            },
            move || conflict_route_menu(agent, route, &base),
        )
        .debug_name("conflict-route"),
    ))
    .style(|s| s.width_full().items_center().gap(8.0))
    .into_any()
}

fn conflict_route_menu(
    agent: perch_agent::Backend,
    route: RwSignal<ConflictRoute>,
    base: &str,
) -> Menu {
    let current = route.get_untracked();
    Menu::new()
        .item(format!("Resolve with {}", agent_name(agent)), move |item| {
            item.checked(current == ConflictRoute::Agent)
                .detail(format!("Merge {base} into this branch here"))
                .action(move || route.set(ConflictRoute::Agent))
        })
        .item("Resolve on GitHub", move |item| {
            item.checked(current == ConflictRoute::GitHub)
                .detail("Open GitHub's conflict editor")
                .action(move || route.set(ConflictRoute::GitHub))
        })
}

/// What the conflicts status says under its title: what the agent does,
/// or is doing, when it is the way they go.
fn conflicts_detail(snapshot: &MergeSnapshot, base: &str) -> String {
    match snapshot.resolver {
        Some(agent) if snapshot.resolving => format!(
            "{} is merging {base} into this branch. Push the merge once it's done.",
            agent_name(agent)
        ),
        Some(agent) if snapshot.conflict_route == ConflictRoute::Agent => format!(
            "{} can merge {base} into this branch here and resolve them, for you to push.",
            agent_name(agent)
        ),
        _ => "Resolve the conflicts on GitHub before continuing.".to_string(),
    }
}

/// The agent that would resolve conflicts, where the reader may push what
/// it makes: Claude or Codex, whichever was last asked, as Write with
/// does.
fn conflicts_agent(app: AppState, state: ReviewState) -> Option<AgentChoice> {
    let viewer = app
        .viewer
        .with(|v| v.as_ref().map(|(login, _)| login.clone()));
    let may = state.item.with(|item| {
        state.pull_request_overview.with(|overview| {
            item.as_ref().is_some_and(|item| {
                crate::items::may_push_pull(item, viewer.as_deref(), overview.as_deref())
            })
        })
    });
    may.then(|| {
        state
            .agent_last
            .get()
            .unwrap_or_else(|| crate::review::threads::defaults(app))
    })
}

/// Whether an agent is resolving this item's conflicts: asked from here,
/// or a run of `CONFLICTS_REQUEST` still going, which outlives a reopen.
fn resolving_conflicts(app: AppState, state: ReviewState) -> bool {
    if state.resolving_conflicts.get() {
        return true;
    }
    let Some(id) = state.item.with(|i| i.as_ref().map(|i| i.id.clone())) else {
        return false;
    };
    app.agent_runs.with(|runs| {
        runs.iter().any(|run| {
            run.item_id.as_deref() == Some(id.as_str())
                && run.status.is_active()
                && run.request.starts_with(CONFLICTS_REQUEST)
        })
    })
}

fn agent_name(agent: perch_agent::Backend) -> &'static str {
    agent.label()
}

/// A button of the merge box, named in the tree for its label.
fn merge_button(
    label: String,
    prominent: bool,
    enabled: impl Fn() -> bool + 'static,
    action: impl Fn() + 'static,
) -> impl View {
    let debug = format!("merge-action:{}", label.replace(' ', "-"));
    action_button(move || label.clone(), debug, prominent, enabled, action)
}

fn merge_decision(snapshot: &MergeSnapshot, pull: &PullRequest, item: &Item) -> MergeDecision {
    decide_merge(merge_facts(
        pull,
        item,
        snapshot.overview.as_deref(),
        snapshot.offline,
        snapshot.historical,
        snapshot.stale,
        snapshot.overview_loading,
    ))
}

fn merge_facts(
    pull: &PullRequest,
    item: &Item,
    overview: Option<&PullRequestOverview>,
    offline: bool,
    historical: bool,
    stale: bool,
    overview_loading: bool,
) -> MergeFacts {
    let heads_match = overview.is_some_and(|overview| {
        pull.head_sha == overview.head_sha
            && item
                .checkout
                .as_ref()
                .is_some_and(|checkout| checkout.head_sha == overview.head_sha)
    });
    let only_ahead = overview.is_some_and(|overview| pull.head_sha == overview.head_sha)
        && item
            .divergence()
            .is_some_and(|d| d.behind == Some(0) && d.ahead.is_some_and(|ahead| ahead > 0));
    MergeFacts {
        offline,
        historical,
        stale,
        diverged: item.divergence().is_some(),
        open: pull.is_open() && !pull.merged,
        draft: pull.draft,
        overview_present: overview.is_some(),
        overview_loading,
        heads_match,
        only_ahead,
        has_write_permission: overview.is_some_and(writes),
        mergeable: overview.map(|overview| overview.mergeable),
        merge_state: overview.map(|overview| overview.merge_state),
        can_bypass: overview.is_some_and(|overview| overview.viewer_can_merge_as_admin),
        has_merge_method: overview.is_some_and(|overview| {
            let methods = overview.allowed_merge_methods;
            methods.merge || methods.squash || methods.rebase
        }),
    }
}

fn decide_merge(facts: MergeFacts) -> MergeDecision {
    if facts.offline {
        return MergeDecision::Disabled(DisabledReason::Offline);
    }
    if facts.historical {
        return MergeDecision::Disabled(DisabledReason::Historical);
    }
    let conflicting = matches!(facts.mergeable, Some(MergeableState::Conflicting))
        || matches!(facts.merge_state, Some(MergeState::Dirty));
    // Unpushed commits don't hide conflicts: pushing them alone leaves
    // GitHub's head conflicting, and the merge can go on top.
    let ahead_of_conflicts = conflicting && facts.only_ahead;
    if facts.stale || (facts.diverged && !ahead_of_conflicts) {
        return MergeDecision::Disabled(DisabledReason::StaleCheckout);
    }
    if !facts.open {
        return MergeDecision::Disabled(DisabledReason::Closed);
    }
    if facts.draft {
        return MergeDecision::Disabled(DisabledReason::Draft);
    }
    if !facts.overview_present {
        return if facts.overview_loading {
            MergeDecision::Loading
        } else {
            MergeDecision::Disabled(DisabledReason::Unknown)
        };
    }
    if !facts.heads_match && !ahead_of_conflicts {
        return MergeDecision::Disabled(DisabledReason::StaleCheckout);
    }
    if !facts.has_write_permission {
        return MergeDecision::Disabled(DisabledReason::NoWritePermission);
    }
    if conflicting {
        return MergeDecision::Conflicts;
    }
    if matches!(facts.mergeable, None | Some(MergeableState::Unknown))
        || matches!(facts.merge_state, None | Some(MergeState::Unknown))
    {
        return MergeDecision::Disabled(DisabledReason::Unknown);
    }
    if matches!(facts.merge_state, Some(MergeState::Draft)) {
        return MergeDecision::Disabled(DisabledReason::Draft);
    }
    if !facts.has_merge_method {
        return MergeDecision::Disabled(DisabledReason::NoMergeMethod);
    }
    if matches!(facts.merge_state, Some(MergeState::Blocked)) {
        return if facts.can_bypass {
            MergeDecision::Bypass
        } else {
            MergeDecision::Disabled(DisabledReason::Requirements)
        };
    }
    MergeDecision::Ready
}

fn request_is_current(state: ReviewState, request: &MergeConfirmation) -> bool {
    if state.merge_pending.get_untracked() {
        return false;
    }
    let Some(pull) = state.pull_request.get_untracked() else {
        return false;
    };
    let Some(item) = state.item.get_untracked() else {
        return false;
    };
    let overview = state.pull_request_overview.get_untracked();
    let facts = merge_facts(
        &pull,
        &item,
        overview.as_deref(),
        state.offline.get_untracked(),
        state.historical_review.with_untracked(Option::is_some),
        state.stale_head.with_untracked(Option::is_some),
        state.pull_request_overview_loading.get_untracked(),
    );
    let decision = decide_merge(facts);
    let Some(overview) = overview else {
        return false;
    };
    request.item_id == item.id
        && request.repo_full_name == pull.repo_full_name
        && request.number == pull.number
        && request.head_sha == overview.head_sha
        && overview.allowed_merge_methods.allows(request.method)
        && matches!(
            (decision, request.bypass_requirements),
            (MergeDecision::Ready, false) | (MergeDecision::Bypass, true)
        )
}

fn ready_detail(overview: Option<&PullRequestOverview>) -> String {
    match overview.map(|overview| overview.merge_state) {
        Some(MergeState::Behind) => {
            "The branch is behind its base. GitHub will verify it again when merging.".to_string()
        }
        Some(MergeState::Unstable) => {
            "Some checks are not successful, but they are not required for this merge.".to_string()
        }
        Some(MergeState::HasHooks) => "GitHub reports this pull request is mergeable.".to_string(),
        _ => "All merge requirements are met.".to_string(),
    }
}

fn blocker_detail(overview: Option<&PullRequestOverview>) -> String {
    let Some(overview) = overview else {
        return "Repository requirements have not been met.".to_string();
    };
    match overview.review_decision {
        Some(ReviewDecision::ReviewRequested) => {
            return "An approving review is still required.".to_string();
        }
        Some(ReviewDecision::ChangesRequested) => {
            return "Requested changes must be resolved.".to_string();
        }
        Some(ReviewDecision::Approved) | None => {}
    }
    match &overview.checks.state {
        CheckState::Failing { workflow } => format!("The {workflow} check has not passed."),
        CheckState::Running => "Required checks are still running.".to_string(),
        _ => "Repository requirements have not been met.".to_string(),
    }
}

fn disabled_copy(
    reason: DisabledReason,
    overview: Option<&PullRequestOverview>,
    diverged: Option<perch_github::Diverged>,
) -> (String, String) {
    let title = "Merging is unavailable".to_string();
    if let (DisabledReason::StaleCheckout, Some(diverged)) = (reason, diverged) {
        return (
            title,
            format!(
                "The worktree isn't at the pull request's head: {}.",
                diverged.label()
            ),
        );
    }
    let detail = match reason {
        DisabledReason::Offline => "Reconnect to GitHub to check and merge this pull request.",
        DisabledReason::Historical => "Return to the current review before merging.",
        DisabledReason::StaleCheckout => {
            "The files on screen do not match GitHub's current pull request head."
        }
        DisabledReason::Closed => "This pull request is no longer open.",
        DisabledReason::Draft => "Mark the pull request ready for review before merging.",
        DisabledReason::Unknown => "GitHub has not reported a safe merge state yet.",
        DisabledReason::NoWritePermission => {
            "Write access to this repository is required to merge."
        }
        DisabledReason::NoMergeMethod => "This repository has no supported merge method enabled.",
        DisabledReason::Requirements => {
            return ("Merging is blocked".to_string(), blocker_detail(overview))
        }
    };
    (title, detail.to_string())
}

fn push_label(ahead: Option<u32>) -> String {
    match ahead {
        Some(1) => "Push 1 commit".to_string(),
        Some(n) if n > 1 => format!("Push {n} commits"),
        _ => "Push".to_string(),
    }
}

fn conflicts_url(html_url: &str) -> Option<String> {
    let html_url = html_url.trim_end_matches('/');
    (!html_url.is_empty()).then(|| format!("{html_url}/conflicts"))
}

fn effective_method(selected: MergeMethod, methods: MergeMethods) -> Option<MergeMethod> {
    if methods.allows(selected) {
        return Some(selected);
    }
    [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase]
        .into_iter()
        .find(|method| methods.allows(*method))
}

fn method_label(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "Create a merge commit",
        MergeMethod::Squash => "Squash and merge",
        MergeMethod::Rebase => "Rebase and merge",
    }
}

fn method_short(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "merge",
        MergeMethod::Squash => "squash",
        MergeMethod::Rebase => "rebase",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::prelude::{KeyState, Modifiers};

    fn check(state: CheckRunState) -> Check {
        Check {
            name: "check".into(),
            workflow: None,
            event: None,
            state,
            description: None,
            url: None,
            started_at: None,
            completed_at: None,
            kind: perch_github::CheckKind::CheckRun,
        }
    }

    fn mergeable_facts() -> MergeFacts {
        MergeFacts {
            offline: false,
            historical: false,
            stale: false,
            diverged: false,
            open: true,
            draft: false,
            overview_present: true,
            overview_loading: false,
            heads_match: true,
            only_ahead: false,
            has_write_permission: true,
            mergeable: Some(MergeableState::Mergeable),
            merge_state: Some(MergeState::Clean),
            can_bypass: false,
            has_merge_method: true,
        }
    }

    fn pull_request(head_sha: &str) -> Arc<PullRequest> {
        Arc::new(PullRequest {
            number: 42,
            title: "Merge support".into(),
            html_url: "https://github.com/acme/repo/pull/42".into(),
            author_login: "octocat".into(),
            author_avatar_url: String::new(),
            head_sha: head_sha.into(),
            head_ref: "feature".into(),
            base_sha: "base".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 1,
            deletions: 0,
            changed_files: 1,
            repo_full_name: "acme/repo".into(),
            head_repo_full_name: Some("acme/repo".into()),
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
        })
    }

    fn review_state(merge_state: MergeState, can_bypass: bool) -> ReviewState {
        let state = ReviewState::new();
        let mut item = Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-42".into();
        item.checkout = Some(perch_github::ItemCheckout {
            branch: "feature".into(),
            base_ref: "main".into(),
            head_sha: "head-42".into(),
            path: "/tmp/perch-merge-test".into(),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.pull_request.set(Some(pull_request("head-42")));
        state
            .pull_request_overview
            .set(Some(Arc::new(PullRequestOverview {
                head_sha: "head-42".into(),
                mergeable: MergeableState::Mergeable,
                merge_state,
                review_decision: (merge_state == MergeState::Blocked)
                    .then_some(ReviewDecision::ReviewRequested),
                viewer_can_merge_as_admin: can_bypass,
                viewer_permission: Some(if can_bypass {
                    RepositoryPermission::Admin
                } else {
                    RepositoryPermission::Write
                }),
                allowed_merge_methods: MergeMethods {
                    merge: true,
                    squash: true,
                    rebase: true,
                },
                checks: Checks {
                    state: CheckState::Passing,
                    url: None,
                    runs: vec![check(CheckRunState::Success)],
                },
                squash_message: CommitMessage::default(),
                merge_message: CommitMessage::default(),
                cross_repository: false,
                maintainer_can_modify: false,
            })));
        state
    }

    /// Nothing to resolve conflicts with: someone else's pull request.
    fn no_resolver() -> Resolver {
        Resolver {
            agent: Memo::new(|_| None),
            busy: Memo::new(|_| false),
            route: RwSignal::new(ConflictRoute::Agent),
        }
    }

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn key(code: Code) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: match code {
                Code::Enter => floem::prelude::Key::Named(floem::prelude::NamedKey::Enter),
                Code::Space => floem::prelude::Key::Character(" ".into()),
                _ => unreachable!("only activation keys are used"),
            },
            code,
            location: Default::default(),
            modifiers: Modifiers::empty(),
            repeat: false,
            is_composing: false,
        })
    }

    #[test]
    fn blocked_merge_requires_an_authorized_explicit_bypass() {
        let mut facts = mergeable_facts();
        facts.merge_state = Some(MergeState::Blocked);
        assert_eq!(
            decide_merge(facts),
            MergeDecision::Disabled(DisabledReason::Requirements)
        );

        facts.can_bypass = true;
        assert_eq!(decide_merge(facts), MergeDecision::Bypass);
    }

    #[test]
    fn conflicts_replace_merge_actions() {
        let mut facts = mergeable_facts();
        facts.mergeable = Some(MergeableState::Conflicting);
        facts.merge_state = Some(MergeState::Dirty);
        assert_eq!(decide_merge(facts), MergeDecision::Conflicts);
        assert_eq!(
            conflicts_url("https://github.com/acme/repo/pull/42/"),
            Some("https://github.com/acme/repo/pull/42/conflicts".to_string())
        );
    }

    #[test]
    fn unpushed_commits_dont_hide_conflicts() {
        let mut facts = mergeable_facts();
        facts.mergeable = Some(MergeableState::Conflicting);
        facts.merge_state = Some(MergeState::Dirty);
        facts.heads_match = false;
        facts.diverged = true;
        assert_eq!(
            decide_merge(facts),
            MergeDecision::Disabled(DisabledReason::StaleCheckout)
        );

        facts.only_ahead = true;
        assert_eq!(decide_merge(facts), MergeDecision::Conflicts);

        facts.mergeable = Some(MergeableState::Mergeable);
        facts.merge_state = Some(MergeState::Clean);
        assert_eq!(
            decide_merge(facts),
            MergeDecision::Disabled(DisabledReason::StaleCheckout)
        );
    }

    /// The reader's worktree with a commit on top of GitHub's conflicting
    /// head is offered the agent's merge, not just Push.
    #[test]
    fn a_worktree_ahead_of_conflicts_can_resolve_them() {
        let state = review_state(MergeState::Dirty, false);
        let pull = pull_request("head-42");
        let mut item = (*state.item.get_untracked().unwrap()).clone();
        item.subject = Some(perch_github::ItemSubject::PullRequest((*pull).clone()));
        let checkout = item.checkout.as_mut().unwrap();
        checkout.head_sha = "head-43".into();
        checkout.divergence = Some(perch_github::Divergence {
            head: "head-43".into(),
            pr_head: "head-42".into(),
            ahead: 1,
            behind: 0,
        });
        let mut overview = (*state.pull_request_overview.get_untracked().unwrap()).clone();
        overview.mergeable = MergeableState::Conflicting;
        let decide = |item: &Item| {
            decide_merge(merge_facts(
                &pull,
                item,
                Some(&overview),
                false,
                false,
                false,
                false,
            ))
        };
        assert_eq!(decide(&item), MergeDecision::Conflicts);

        // Behind GitHub's head, a merge here couldn't be pushed.
        item.checkout
            .as_mut()
            .unwrap()
            .divergence
            .as_mut()
            .unwrap()
            .behind = 1;
        assert_eq!(
            decide(&item),
            MergeDecision::Disabled(DisabledReason::StaleCheckout)
        );
    }

    #[test]
    fn a_diverged_worktree_says_how_in_the_reason() {
        let diverged = perch_github::Diverged {
            ahead: Some(1),
            behind: Some(0),
        };
        let (_, detail) = disabled_copy(DisabledReason::StaleCheckout, None, Some(diverged));
        assert_eq!(
            detail,
            "The worktree isn't at the pull request's head: 1 unpushed."
        );
        let (_, detail) = disabled_copy(DisabledReason::StaleCheckout, None, None);
        assert_eq!(
            detail,
            "The files on screen do not match GitHub's current pull request head."
        );
    }

    #[test]
    fn stale_and_unknown_heads_never_offer_merge() {
        let mut facts = mergeable_facts();
        facts.heads_match = false;
        assert_eq!(
            decide_merge(facts),
            MergeDecision::Disabled(DisabledReason::StaleCheckout)
        );

        facts.heads_match = true;
        facts.mergeable = Some(MergeableState::Unknown);
        assert_eq!(
            decide_merge(facts),
            MergeDecision::Disabled(DisabledReason::Unknown)
        );
    }

    #[test]
    fn selected_method_falls_back_only_to_an_enabled_method() {
        let methods = MergeMethods {
            merge: true,
            squash: false,
            rebase: false,
        };
        assert_eq!(
            effective_method(MergeMethod::Squash, methods),
            Some(MergeMethod::Merge)
        );
    }

    #[test]
    fn check_rows_include_status_descriptions_and_compact_durations() {
        let mut detailed = check(CheckRunState::Success);
        detailed.description = Some("Contributor License Agreement is signed.".into());
        detailed.started_at = Some("2026-09-20T09:00:00Z".parse().unwrap());
        detailed.completed_at = Some("2026-09-20T09:10:42Z".parse().unwrap());

        assert_eq!(
            check_label(&detailed),
            "check — Contributor License Agreement is signed."
        );
        assert_eq!(check_status_label(&detailed), "Passed in 10m");
        assert_eq!(compact_duration(6), "6s");
        assert_eq!(compact_duration(3_900), "1h 5m");
    }

    #[test]
    fn rendered_box_supports_keyboard_bypass_confirmation() {
        let root = TestRoot::new();
        let state = review_state(MergeState::Blocked, true);
        let view = MergeBox::new(crate::state::AppState::new(), state)
            .section(state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        let bypass = named(harness.root_id(), "merge-bypass").unwrap();
        bypass.request_focus();
        harness.rebuild();
        assert!(harness.is_focused(bypass));
        harness.dispatch_event(key(Code::Space));
        harness.rebuild();
        harness.paint();

        let action = named(
            harness.root_id(),
            "merge-action:Bypass-rules-and-merge-(squash)",
        )
        .unwrap();
        action.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(Code::Enter));
        harness.rebuild();
        harness.paint();

        assert!(named(harness.root_id(), "merge-action:Confirm-bypass-and-merge").is_some());
    }

    /// A worktree with commits its pull request lacks can't merge, and the
    /// merge box pushes them instead.
    #[test]
    fn unpushed_commits_turn_merge_into_push() {
        use crate::backend::Command;

        let root = TestRoot::new();
        let state = review_state(MergeState::Clean, false);
        state.item.update(|slot| {
            let mut item = (**slot.as_ref().unwrap()).clone();
            item.subject = Some(perch_github::ItemSubject::PullRequest(
                (*pull_request("head-42")).clone(),
            ));
            let checkout = item.checkout.as_mut().unwrap();
            checkout.head_sha = "head-43".into();
            checkout.divergence = Some(perch_github::Divergence {
                head: "head-43".into(),
                pr_head: "head-42".into(),
                ahead: 2,
                behind: 0,
            });
            *slot = Some(Arc::new(item));
        });
        let (backend, mut commands) = Backend::recording();
        let view = merge_section(
            state,
            Rc::new(backend),
            RwSignal::new(MergeMethod::Squash),
            RwSignal::new(false),
            RwSignal::new(None::<MergeConfirmation>),
            MessageDraft::new(),
            Memo::new(|_| true),
            Memo::new(|_| false),
            no_resolver(),
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        let push =
            named(harness.root_id(), "merge-action:Push-2-commits").expect("Push in the merge box");
        assert!(named(harness.root_id(), "merge-action:Merge-unavailable").is_none());
        push.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(Code::Enter));
        harness.rebuild();
        harness.paint();

        let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(
            sent.iter()
                .any(|c| matches!(c, Command::PushBranch { item_id } if item_id == "item-42")),
            "{sent:?}"
        );
        assert!(state.push_pending.get_untracked());
        assert!(named(harness.root_id(), "merge-action:Pushing…").is_some());

        // A refused push says why, and Push comes back.
        state.push_pending.set(false);
        state
            .push_error
            .set(Some("The remote has commits this branch doesn't.".into()));
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "merge-push-error").is_some());
        assert!(named(harness.root_id(), "merge-action:Push-2-commits").is_some());
    }

    /// A worktree whose pull request's head isn't here can't merge, and
    /// the merge box updates the branch instead; one with commits of its
    /// own as well offers Rebase in its place.
    #[test]
    fn a_pull_request_head_not_fetched_turns_merge_into_update_branch() {
        use crate::backend::Command;

        let root = TestRoot::new();
        let state = review_state(MergeState::Clean, false);
        state.item.update(|slot| {
            let mut item = (**slot.as_ref().unwrap()).clone();
            item.subject = Some(perch_github::ItemSubject::PullRequest(
                (*pull_request("head-42")).clone(),
            ));
            item.checkout.as_mut().unwrap().head_sha = "head-41".into();
            *slot = Some(Arc::new(item));
        });
        let (backend, mut commands) = Backend::recording();
        let view = merge_section(
            state,
            Rc::new(backend),
            RwSignal::new(MergeMethod::Squash),
            RwSignal::new(false),
            RwSignal::new(None::<MergeConfirmation>),
            MessageDraft::new(),
            Memo::new(|_| false),
            Memo::new(|_| false),
            no_resolver(),
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        let update = named(harness.root_id(), "merge-action:Update-branch")
            .expect("Update branch in the merge box");
        update.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(Code::Enter));
        harness.rebuild();
        harness.paint();

        let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(
            sent.iter()
                .any(|c| matches!(c, Command::UpdateBranch { item_id, rebase: false } if item_id == "item-42")),
            "{sent:?}"
        );
        assert!(named(harness.root_id(), "merge-action:Updating…").is_some());

        state.update_pending.set(false);
        state.item.update(|slot| {
            let mut item = (**slot.as_ref().unwrap()).clone();
            item.checkout.as_mut().unwrap().divergence = Some(perch_github::Divergence {
                head: "head-41".into(),
                pr_head: "head-42".into(),
                ahead: 1,
                behind: 1,
            });
            *slot = Some(Arc::new(item));
        });
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "merge-action:Update-branch").is_none());
        let rebase =
            named(harness.root_id(), "merge-action:Rebase").expect("Rebase in the merge box");
        rebase.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(Code::Enter));
        harness.rebuild();
        harness.paint();
        let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(
            sent.iter()
                .any(|c| matches!(c, Command::UpdateBranch { item_id, rebase: true } if item_id == "item-42")),
            "{sent:?}"
        );
        assert!(named(harness.root_id(), "merge-action:Rebasing…").is_some());
    }

    /// Conflicts go to an agent by default where one may work, held while
    /// it runs, and to GitHub when the reader picks it beside the button or
    /// no agent may work here.
    #[test]
    fn conflicts_are_resolved_with_an_agent_by_default() {
        use crate::backend::Command;

        let root = TestRoot::new();
        let state = review_state(MergeState::Dirty, false);
        state.pull_request_overview.update(|overview| {
            let mut dirty = (**overview.as_ref().unwrap()).clone();
            dirty.mergeable = MergeableState::Conflicting;
            *overview = Some(Arc::new(dirty));
        });
        let may_work = RwSignal::new(true);
        let resolver = Resolver {
            agent: Memo::new(move |_| {
                may_work.get().then(|| {
                    (
                        perch_agent::Backend::ClaudeCode,
                        "opus".to_string(),
                        perch_agent::Effort::default(),
                    )
                })
            }),
            busy: Memo::new(move |_| state.resolving_conflicts.get()),
            route: RwSignal::new(ConflictRoute::Agent),
        };
        let (backend, mut commands) = Backend::recording();
        let view = merge_section(
            state,
            Rc::new(backend),
            RwSignal::new(MergeMethod::Squash),
            RwSignal::new(false),
            RwSignal::new(None::<MergeConfirmation>),
            MessageDraft::new(),
            Memo::new(|_| false),
            Memo::new(|_| false),
            resolver,
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        let resolve = named(
            harness.root_id(),
            "merge-action:Resolve-conflicts-with-Claude",
        )
        .expect("the agent first");
        assert!(named(harness.root_id(), "conflict-route").is_some());
        resolve.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(Code::Enter));
        harness.rebuild();
        harness.paint();

        let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(
            sent.iter().any(|c| matches!(
                c,
                Command::ResolveConflicts {
                    item_id,
                    backend: perch_agent::Backend::ClaudeCode,
                    ..
                } if item_id == "item-42"
            )),
            "{sent:?}"
        );
        assert!(state.resolving_conflicts.get_untracked());
        assert!(named(harness.root_id(), "merge-action:Resolving-conflicts…").is_some());
        assert!(named(harness.root_id(), "conflict-route").is_none());

        // Done, and GitHub picked: the choice is back, going to GitHub.
        state.resolving_conflicts.set(false);
        resolver.route.set(ConflictRoute::GitHub);
        harness.rebuild();
        harness.paint();
        assert!(named(
            harness.root_id(),
            "merge-action:Resolve-conflicts-on-GitHub"
        )
        .is_some());
        assert!(named(harness.root_id(), "conflict-route").is_some());

        // No agent may work here: GitHub alone, with nothing to pick.
        may_work.set(false);
        harness.rebuild();
        harness.paint();
        assert!(named(
            harness.root_id(),
            "merge-action:Resolve-conflicts-on-GitHub"
        )
        .is_some());
        assert!(named(harness.root_id(), "conflict-route").is_none());
    }

    /// A draft's merge box marks it ready for review, for a reader who may,
    /// and holds the button until the pull request has been read again.
    #[test]
    fn a_draft_is_marked_ready_from_the_merge_box() {
        use crate::backend::Command;

        let root = TestRoot::new();
        let state = review_state(MergeState::Draft, false);
        state.pull_request.update(|pull| {
            let mut draft = (**pull.as_ref().unwrap()).clone();
            draft.draft = true;
            *pull = Some(Arc::new(draft));
        });
        let may_set_draft = RwSignal::new(false);
        let (backend, mut commands) = Backend::recording();
        let view = merge_section(
            state,
            Rc::new(backend),
            RwSignal::new(MergeMethod::Squash),
            RwSignal::new(false),
            RwSignal::new(None::<MergeConfirmation>),
            MessageDraft::new(),
            Memo::new(|_| false),
            Memo::new(move |_| may_set_draft.get()),
            no_resolver(),
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        // Nobody who may change it: nothing to press.
        assert!(named(harness.root_id(), "merge-action:Merge-unavailable").is_some());
        assert!(named(harness.root_id(), "merge-action:Ready-for-review").is_none());

        may_set_draft.set(true);
        harness.rebuild();
        harness.paint();
        let ready =
            named(harness.root_id(), "merge-action:Ready-for-review").expect("Ready for review");
        ready.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(Code::Enter));
        harness.rebuild();
        harness.paint();

        let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(
            sent.iter().any(|c| matches!(
                c,
                Command::SetDraft { item_id, draft: false } if item_id == "item-42"
            )),
            "{sent:?}"
        );
        assert_eq!(state.draft_pending.get_untracked(), Some(false));
        assert!(named(harness.root_id(), "merge-action:Marking-ready-for-review…").is_some());

        // GitHub's answer is in, the merge state not yet: still held.
        state.pull_request.update(|pull| {
            let mut ready = (**pull.as_ref().unwrap()).clone();
            ready.draft = false;
            *pull = Some(Arc::new(ready));
        });
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "merge-action:Marking-ready-for-review…").is_some());

        // Read again: the merge box is a merge box.
        state.pull_request_overview.update(|overview| {
            let mut fresh = (**overview.as_ref().unwrap()).clone();
            fresh.merge_state = MergeState::Clean;
            *overview = Some(Arc::new(fresh));
        });
        state.draft_pending.set(None);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "merge-action:Squash-and-merge").is_some());
    }

    /// Squash confirms with GitHub's message to start with; what is typed
    /// is what goes, and it outlives the confirmation for the same head.
    #[test]
    fn a_squash_confirms_an_editable_commit_message() {
        use crate::backend::Command;

        let root = TestRoot::new();
        let state = review_state(MergeState::Clean, false);
        state.pull_request_overview.update(|overview| {
            let mut edited = (**overview.as_ref().unwrap()).clone();
            edited.squash_message = CommitMessage {
                title: "Merge support (#42)".into(),
                body: "* Add the merge box".into(),
            };
            *overview = Some(Arc::new(edited));
        });
        let (backend, mut commands) = Backend::recording();
        let selected_method = RwSignal::new(MergeMethod::Squash);
        let bypass = RwSignal::new(false);
        let confirmation = RwSignal::new(None::<MergeConfirmation>);
        let message = MessageDraft::new();
        let view = merge_section(
            state,
            Rc::new(backend),
            selected_method,
            bypass,
            confirmation,
            message,
            Memo::new(|_| false),
            Memo::new(|_| false),
            no_resolver(),
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        let press = |harness: &mut HeadlessHarness, name: &str| {
            let id = named(harness.root_id(), name).unwrap_or_else(|| panic!("no view {name}"));
            id.request_focus();
            harness.rebuild();
            harness.dispatch_event(key(Code::Enter));
            harness.rebuild();
            harness.paint();
        };
        harness.rebuild();
        harness.paint();

        press(&mut harness, "merge-action:Squash-and-merge");
        assert!(named(harness.root_id(), "merge-commit-title").is_some());
        assert!(named(harness.root_id(), "merge-commit-body").is_some());
        assert_eq!(message.title.get_untracked(), "Merge support (#42)");
        assert_eq!(message.body.get_untracked(), "* Add the merge box");

        // Typed, put away, opened again: the typing stays.
        message.title.set("Add a merge box (#42)".into());
        press(&mut harness, "merge-action:Cancel");
        press(&mut harness, "merge-action:Squash-and-merge");
        assert_eq!(message.title.get_untracked(), "Add a merge box (#42)");

        // A blank title can't be confirmed.
        message.title.set("  ".into());
        press(&mut harness, "merge-action:Confirm-merge");
        assert!(std::iter::from_fn(|| commands.try_recv().ok())
            .all(|c| !matches!(c, Command::MergePullRequest { .. })));

        message.title.set("Add a merge box (#42) ".into());
        message.body.set(String::new());
        harness.rebuild();
        harness.paint();
        press(&mut harness, "merge-action:Confirm-merge");
        let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(
            sent.iter().any(|c| matches!(
                c,
                Command::MergePullRequest { method: MergeMethod::Squash, message: Some(m), .. }
                    if m.title == "Add a merge box (#42)" && m.body.is_empty()
            )),
            "{sent:?}"
        );
    }

    /// GitHub's message read again, as it is after the description is
    /// edited, reaches a draft the reader left as GitHub's, and not one
    /// they typed in.
    #[test]
    fn a_squash_message_left_as_githubs_follows_githubs() {
        let root = TestRoot::new();
        let state = review_state(MergeState::Clean, false);
        let read = |title: &str, body: &str| {
            state.pull_request_overview.update(|overview| {
                let mut read = (**overview.as_ref().unwrap()).clone();
                read.squash_message = CommitMessage {
                    title: title.into(),
                    body: body.into(),
                };
                *overview = Some(Arc::new(read));
            });
        };
        read("Merge support (#42)", "* Add the merge box");
        let message = MessageDraft::new();
        let view = merge_section(
            state,
            Rc::new(Backend::detached()),
            RwSignal::new(MergeMethod::Squash),
            RwSignal::new(false),
            RwSignal::new(None::<MergeConfirmation>),
            message,
            Memo::new(|_| false),
            Memo::new(|_| false),
            no_resolver(),
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        let press = |harness: &mut HeadlessHarness, name: &str| {
            harness.rebuild();
            harness.paint();
            let id = named(harness.root_id(), name).unwrap_or_else(|| panic!("no view {name}"));
            id.request_focus();
            harness.rebuild();
            harness.dispatch_event(key(Code::Enter));
            harness.rebuild();
            harness.paint();
        };

        press(&mut harness, "merge-action:Squash-and-merge");
        assert_eq!(message.body.get_untracked(), "* Add the merge box");

        // Read again with the description edited, the draft untouched.
        press(&mut harness, "merge-action:Cancel");
        read("Merge support (#42)", "* Add the merge box\n* Say why");
        press(&mut harness, "merge-action:Squash-and-merge");
        assert_eq!(message.title.get_untracked(), "Merge support (#42)");
        assert_eq!(
            message.body.get_untracked(),
            "* Add the merge box\n* Say why"
        );

        // Typed in, then read again: the typing stays.
        message.title.set("Add a merge box (#42)".into());
        press(&mut harness, "merge-action:Cancel");
        read("Merge support (#42)", "* Something else");
        press(&mut harness, "merge-action:Squash-and-merge");
        assert_eq!(message.title.get_untracked(), "Add a merge box (#42)");
        assert_eq!(
            message.body.get_untracked(),
            "* Add the merge box\n* Say why"
        );
    }

    #[test]
    fn a_rebase_has_no_message_to_edit() {
        assert!(!takes_message(MergeMethod::Rebase));
        assert!(takes_message(MergeMethod::Squash));
        assert!(takes_message(MergeMethod::Merge));
    }

    /// Merged elsewhere while GitHub's last word on the rules still says
    /// blocked: the box says merged, not blocked, and offers no merge.
    #[test]
    fn a_merged_pull_request_says_merged_and_offers_no_merge() {
        let root = TestRoot::new();
        let state = review_state(MergeState::Blocked, false);
        let mut merged = (*pull_request("head-42")).clone();
        merged.state = "closed".into();
        merged.merged = true;
        merged.merged_by = Some("criccomini".into());
        state.pull_request.set(Some(Arc::new(merged.clone())));
        let view = MergeBox::new(crate::state::AppState::new(), state)
            .section(state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        assert!(named(harness.root_id(), "merge-finished").is_some());
        assert!(named(harness.root_id(), "merge-action:Merge-unavailable").is_none());
        assert_eq!(
            finished_copy(&merged),
            Some((
                SubjectState::PrMerged,
                "Merged".to_string(),
                "criccomini merged this into main.".to_string()
            ))
        );

        merged.merged = false;
        assert_eq!(
            finished_copy(&merged).map(|(state, title, _)| (state, title)),
            Some((SubjectState::PrClosed, "Closed".to_string()))
        );
        assert_eq!(finished_copy(&pull_request("head-42")), None);
    }
}
