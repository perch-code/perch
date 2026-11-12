//! Agent activity: every item's runs in the dropdown from the bar's
//! Agents button (`dropdown`), the open item's first; its reviews in the
//! Review popover (`reviews`); what a run did, in a sheet over the window
//! (`detail`); and the toast a run leaves as it finishes.
//!
//! A run is brought into view where it is read: a question in its thread
//! on Conversation, where its answer is, and a reviewer in its review in
//! the Review popover, where the findings are. See `reveal`.

pub mod model;

pub mod detail;
pub mod dropdown;
pub(crate) mod reviews;

use std::rc::Rc;
use std::time::Duration;

use floem::event::EventPropagation;
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Effect, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::doc::CardId;
use crate::review::model::{ReviewFindingFilter, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, icon_action};

use model::{AgentRetryTarget, AgentRunStatus, AgentRunSummary};

/// The clock that keeps the elapsed times moving runs while there is work
/// to time, and the run asked for from elsewhere brought into view once it
/// and its item are here. Once, from the window.
pub fn wire(state: AppState, review: ReviewState, backend: Rc<Backend>) {
    Effect::new(move |_| {
        let active = state
            .agent_runs
            .with(|runs| runs.iter().any(|run| run.status.is_active()));
        if active && !state.agents_clock_armed.get_untracked() {
            arm_clock(state);
        }
    });
    // Asked for by the dropdown, a toast, or a navigation that opened the
    // run's item: watched until the run is loaded and its item is the one
    // open, as after a navigation the run may land after the item.
    Effect::new(move |_| {
        let Some(run_id) = review.pending_run_reveal.get() else {
            return;
        };
        let Some(item_id) = review
            .item
            .with(|item| item.as_ref().map(|item| item.id.clone()))
        else {
            return;
        };
        let Some(run) = state.agent_run(&run_id) else {
            return;
        };
        if run.item_id.as_deref() != Some(item_id.as_str()) {
            return;
        }
        review.pending_run_reveal.set(None);
        reveal(state, review, &backend, &run);
    });
    // A reviewer's run revealed before its review's record came, as one
    // from an item first opened from a toast or the dropdown is, opens
    // its review under the stand-in key a review has while only its runs
    // are known. The record keys it by its own id, so once that comes the
    // open review is that, or no line would be open.
    Effect::new(move |_| {
        let Some(attempt) = review.review_expanded.with(|key| {
            key.as_deref()
                .and_then(|key| key.strip_prefix("attempt:"))
                .map(str::to_string)
        }) else {
            return;
        };
        let Some(item_id) = review
            .item
            .with(|item| item.as_ref().map(|item| item.id.clone()))
        else {
            return;
        };
        let groups = state.review_groups_for(&item_id);
        if let Some(summary) = model::review_group_for_attempt(&groups, &attempt) {
            review.review_expanded.set(Some(summary.group.id.clone()));
        }
    });
}

/// Bring a run on the open item into view where it is read. A reviewer's
/// run opens the Review popover with its review open; a question's goes to
/// its thread on Conversation, which flashes. A run with no thread, one
/// whose owner is gone or one that never had a discussion, has nowhere on
/// the page to go, so what it did comes up in its sheet instead.
pub(crate) fn reveal(app: AppState, review: ReviewState, backend: &Backend, run: &AgentRunSummary) {
    if let Some(attempt) = &run.review_attempt_id {
        let key = run
            .item_id
            .as_deref()
            .and_then(|item_id| {
                model::review_group_for_attempt(&app.review_groups_for(item_id), attempt)
                    .map(|summary| summary.group.id.clone())
            })
            .unwrap_or_else(|| format!("attempt:{attempt}"));
        open_review(review, key);
        return;
    }
    match (&run.item_id, &run.thread_id) {
        (Some(item_id), Some(thread_id)) => show_thread(app, review, item_id, thread_id),
        _ => show_output(app, review, backend, &run.id),
    }
}

/// Open the Review popover with the agents' review `key` open in it, which
/// folds the reader's own review to its line.
pub(crate) fn open_review(review: ReviewState, key: String) {
    floem::reactive::Effect::batch(|| {
        review.review_expanded.set(Some(key));
        crate::review::review_menu::open(review, None);
    });
}

/// Bring a thread into view on Conversation, flashing. One the findings'
/// Open filter hides, as it does a dismissed or fixed finding's, turns the
/// filter to All first.
pub(crate) fn show_thread(app: AppState, review: ReviewState, item_id: &str, thread_id: &str) {
    if review
        .hidden_review_threads
        .with_untracked(|hidden| hidden.contains(thread_id))
    {
        review.apply_review_finding_filter(
            ReviewFindingFilter::All,
            &app.review_findings_for(item_id),
        );
    }
    crate::review::notified::go_to(review, CardId::Thread(thread_id.to_string()));
}

/// Put up a run's raw output in its sheet, asking for what it did the
/// first time.
pub(crate) fn show_output(app: AppState, review: ReviewState, backend: &Backend, run_id: &str) {
    if app
        .agent_run_events
        .with_untracked(|all| !all.contains_key(run_id))
    {
        app.set_agent_run_events_loading(run_id);
        backend.load_agent_run_detail(run_id);
    }
    review.agent_output.set(Some(run_id.to_string()));
}

/// Ask for an item's runs, the first page, the first time they are
/// wanted: as the dropdown or the Review popover opens on it.
pub(crate) fn load_item_runs(app: AppState, backend: &Backend, item_id: &str) {
    if !app
        .item_runs
        .with_untracked(|pages| pages.contains_key(item_id))
    {
        app.load_item_runs(backend, item_id, false);
    }
}

/// Cancel, Stop or Retry as the run allows: a glyph, shown while its row
/// is under the pointer.
pub(super) fn run_actions(
    hovered: RwSignal<bool>,
    backend: Rc<Backend>,
    run: &AgentRunSummary,
) -> impl IntoView {
    use crate::review::cards::hover_action;
    let id = run.id.clone();
    match run.status {
        AgentRunStatus::Queued => hover_action(
            hovered,
            SidebarIcon::Close,
            "Cancel",
            "agent-run cancel",
            Rc::new(move || backend.cancel_run(&id)),
        )
        .into_any(),
        status if status.may_stop() => hover_action(
            hovered,
            SidebarIcon::Stop,
            "Stop",
            "agent-run stop",
            Rc::new(move || backend.cancel_run(&id)),
        )
        .into_any(),
        status if status.may_retry() => {
            let retry: Rc<dyn Fn()> = match run.retry_target() {
                AgentRetryTarget::ReviewAttempt(attempt_id) => {
                    Rc::new(move || backend.retry_review_attempt(&attempt_id))
                }
                AgentRetryTarget::Run(id) => Rc::new(move || backend.retry_run(&id)),
            };
            hover_action(
                hovered,
                SidebarIcon::Refresh,
                "Retry",
                "agent-run retry",
                retry,
            )
            .into_any()
        }
        _ => Empty::new().into_any(),
    }
}

pub(super) fn text_action(
    text: impl Fn() -> String + 'static,
    prominent: bool,
    action: impl Fn() + 'static,
) -> impl IntoView {
    let action: Rc<dyn Fn()> = Rc::new(action);
    let click = action.clone();
    Container::new(Label::derived(text).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(theme::weight_emphasis())
            .color(if prominent {
                theme::accent()
            } else {
                theme::fg_muted()
            })
            .selectable(false)
            .pointer_events_none()
    }))
    .on_event_stop(el::Click, move |_, _| click())
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            action();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(move |s| {
        let base = clickable(s)
            .height(27.0)
            .items_center()
            .padding_horiz(8.0)
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()));
        if prominent {
            base.background(theme::accent_subtle())
                .hover(|s| s.background(theme::badge_bg()))
        } else {
            base.hover(|s| s.background(theme::canvas_hover()))
        }
    })
}

pub(super) fn status_color(status: AgentRunStatus) -> fn() -> Color {
    match status {
        AgentRunStatus::Starting | AgentRunStatus::Running => theme::running,
        AgentRunStatus::Completed => theme::open,
        AgentRunStatus::Failed => theme::closed,
        AgentRunStatus::Stopped => theme::fg_subtle,
        AgentRunStatus::Queued | AgentRunStatus::Stopping | AgentRunStatus::Interrupted => {
            theme::draft_yellow
        }
    }
}

/// A dot that beats with the agents' clock while there is work: full on
/// even seconds, faded on odd, easing between. It holds still once the
/// clock stops.
pub(super) fn live_dot(app: AppState, size: f64) -> impl IntoView {
    floem::views::Empty::new().style(move |s| {
        let faded = app.agents_clock_armed.get() && app.now.get().as_second() % 2 != 0;
        let color = theme::running();
        s.size(size, size)
            .border_radius(size / 2.0)
            .flex_shrink(0.0_f32)
            .background(if faded { color.with_alpha(0.35) } else { color })
            .transition_background(floem::style::Transition::ease_in_out(
                Duration::from_millis(900),
            ))
    })
}

/// Where a queued run is in the queue, from one.
pub(super) fn queue_position(state: AppState, id: &str) -> Option<usize> {
    state.agent_runs.with(|runs| {
        let mut queued: Vec<_> = runs
            .iter()
            .filter(|run| run.status == AgentRunStatus::Queued)
            .collect();
        queued.sort_by_key(|run| run.queued_at);
        queued
            .iter()
            .position(|run| run.id == id)
            .map(|position| position + 1)
    })
}

fn arm_clock(state: AppState) {
    if state.agents_clock_armed.get_untracked() {
        return;
    }
    state.agents_clock_armed.set(true);
    tick_clock(state);
}

fn tick_clock(state: AppState) {
    floem::action::exec_after(Duration::from_secs(1), move |_| {
        let active = state
            .agent_runs
            .with_untracked(|runs| runs.iter().any(|run| run.status.is_active()));
        if active {
            state.now.set(jiff::Timestamp::now());
            tick_clock(state);
        } else {
            state.agents_clock_armed.set(false);
        }
    });
}

/// A terminal transition stays lightweight in the popup: enough context to
/// identify it, and the action that goes to the durable detail, which the
/// whole card runs when clicked.
pub(crate) fn completion_toast(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    token: u64,
    run: AgentRunSummary,
) -> (impl IntoView, Rc<dyn Fn()>) {
    let color = status_color(run.status);
    let title = format!(
        "{} {}",
        run.backend_label(),
        run.status.label().to_lowercase()
    );
    let context = {
        let group = run.group_label();
        if group.is_empty() {
            run.purpose.label().to_string()
        } else {
            format!("{} · {group}", run.purpose.label())
        }
    };
    let open: Rc<dyn Fn()> = Rc::new(move || {
        state.dismiss_toast(token);
        dropdown::go_to_run(state, review, &backend, &run);
    });
    let dismiss = icon_action(
        SidebarIcon::Close,
        "Dismiss agent completion",
        || true,
        move || state.dismiss_toast(token),
    );
    let body = Stack::horizontal((
        crate::ui::toast::dot(color),
        Stack::vertical((
            Label::derived(move || title.clone()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg())
                    .selectable(false)
            }),
            Label::derived(move || context.clone()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .text_ellipsis()
                    .min_width(0.0)
                    .selectable(false)
            }),
        ))
        .style(|s| s.flex_col().gap(3.0).min_width(0.0).flex_grow(1.0_f32)),
        dismiss,
    ))
    .style(|s| s.items_center().width_full().gap(9.0));
    (body, open)
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};

    fn running_record() -> perch_github::RunRecord {
        perch_github::RunRecord {
            id: "stream".into(),
            owner: perch_github::RunOwner::Unavailable {
                legacy_thread_id: None,
            },
            backend: "claude-code".into(),
            requested_model: "sonnet".into(),
            model: "sonnet".into(),
            requested_effort: "high".into(),
            effort: "high".into(),
            original_request: "Explain the scheduler".into(),
            status: perch_github::RunStatus::Running,
            queued_at: "2026-09-19T10:00:00Z".parse().unwrap(),
            started_at: Some("2026-09-19T10:00:01Z".parse().unwrap()),
            latest_activity: Some("Reading the queue".into()),
            last_event_sequence: 1,
            ..perch_github::RunRecord::default()
        }
    }

    /// The clock that moves the elapsed times starts when work arrives,
    /// wherever the runs are on screen.
    #[test]
    fn the_clock_arms_when_work_arrives() {
        let root = TestRoot::new();
        let app = AppState::new();
        let review = ReviewState::new();
        let backend = Rc::new(Backend::detached());
        let view = crate::ui::app_view(app, review, backend);
        let mut harness = HeadlessHarness::new_with_size(root, view, 900.0, 700.0);
        harness.rebuild();
        assert!(!app.agents_clock_armed.get());

        app.merge_agent_runs([perch_github::RunSummary::from(&running_record())]);
        harness.rebuild();
        assert!(app.agents_clock_armed.get());
    }
}

/// What the dropdown's and the popover's tests share: the pull request
/// window, with a backend whose commands the test can read, and runs,
/// reviews and threads to put on it.
#[cfg(test)]
pub(crate) mod test_support {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::SignalUpdate;

    use crate::backend::{Backend, Command};
    use crate::review::model::{Phase, ReviewState};
    use crate::state::AppState;

    pub(crate) use crate::review::header::tests::{named, rect};

    pub(crate) fn at(s: &str) -> jiff::Timestamp {
        s.parse().unwrap()
    }

    /// A question on `item` asked at `minute` past ten, in thread `t-{id}`.
    pub(crate) fn record(
        id: &str,
        item: &str,
        status: perch_github::RunStatus,
        minute: u32,
    ) -> perch_github::RunRecord {
        let when = at(&format!("2026-09-19T10:{minute:02}:00Z"));
        perch_github::RunRecord {
            id: id.into(),
            owner: perch_github::RunOwner::Discussion {
                item_id: item.into(),
                thread_id: format!("t-{id}"),
                request_id: format!("r-{id}"),
            },
            backend: "claude-code".into(),
            model: "sonnet".into(),
            effort: "high".into(),
            original_request: format!("Question {id}"),
            status,
            queued_at: when,
            started_at: Some(when),
            ended_at: (!matches!(
                status,
                perch_github::RunStatus::Running | perch_github::RunStatus::Queued
            ))
            .then_some(when),
            ..perch_github::RunRecord::default()
        }
    }

    /// A reviewer's run on `pr` for attempt `attempt`.
    pub(crate) fn reviewer(
        run: &str,
        attempt: &str,
        status: perch_github::RunStatus,
        minute: u32,
    ) -> perch_github::RunRecord {
        perch_github::RunRecord {
            owner: perch_github::RunOwner::Review {
                item_id: "pr".into(),
                reviewer_run_id: attempt.into(),
            },
            purpose: perch_github::comments::RunPurpose::WorktreeReview,
            ..record(run, "pr", status, minute)
        }
    }

    pub(crate) fn add(app: AppState, records: &[perch_github::RunRecord]) {
        app.merge_agent_runs(records.iter().map(perch_github::RunSummary::from));
    }

    /// Review `number` of `pr`, `group-{number}`, with an attempt for each
    /// of `attempts`, later the higher its number.
    pub(crate) fn review(
        number: u64,
        attempts: &[&str],
    ) -> crate::review::model::ReviewGroupSummary {
        let mut summary = super::model::tests::review_summary(&format!("group-{number}"), attempts);
        summary.group.number = number;
        let when = at(&format!("2026-09-19T{:02}:00:00Z", 8 + number));
        summary.group.created_at = when;
        summary.group.started_at = Some(when);
        summary
    }

    pub(crate) fn set_reviews(
        app: AppState,
        reviews: Vec<crate::review::model::ReviewGroupSummary>,
    ) {
        app.review_groups.update(|all| {
            all.insert("pr".into(), Arc::new(reviews));
        });
    }

    /// A thread on `src/a.rs` at `line` saying `body`.
    pub(crate) fn thread(id: &str, body: &str, line: u32) -> perch_github::CommentThread {
        let when = at("2026-09-19T10:00:00Z");
        perch_github::CommentThread {
            id: id.into(),
            worktree_id: "pr".into(),
            origin: perch_github::ThreadOrigin::Local,
            github_id: None,
            anchor: Some(perch_github::Anchor {
                path: "src/a.rs".into(),
                start_line: line,
                line,
                side: perch_github::Side::Right,
                commit_sha: "head".into(),
            }),
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at: when,
            messages: vec![perch_github::CommentMessage {
                id: format!("{id}-m"),
                thread_id: id.into(),
                origin: perch_github::ThreadOrigin::Local,
                author: perch_github::Author::User,
                body: body.into(),
                created_at: when,
                run: None,
                github_id: None,
                node_id: None,
                html_url: None,
                diff_hunk: None,
                reactions: Default::default(),
            }],
        }
    }

    pub(crate) fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..3 {
            harness.rebuild();
            harness.paint();
        }
    }

    pub(crate) fn click(harness: &mut HeadlessHarness, name: &str) {
        let at = rect(harness, name).center();
        harness.click(at.x, at.y);
        settle(harness);
    }

    pub(crate) fn shown(harness: &HeadlessHarness, name: &str) -> bool {
        named(harness.root_id(), name).is_some_and(|id| id.get_visual_rect_no_clip().height() > 0.0)
    }

    /// The window on an open pull request, `pr`, with a backend that
    /// records what it is asked.
    pub(crate) fn window() -> (
        HeadlessHarness,
        AppState,
        ReviewState,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) {
        let app = AppState::new();
        let state = ReviewState::new();
        let pull = crate::review::header::tests::pull_request(812);
        let mut item = perch_github::Item::new("slatedb/slatedb", at("2026-09-20T12:00:00Z"));
        item.id = "pr".into();
        item.subject = Some(perch_github::ItemSubject::PullRequest(pull.clone()));
        app.items.set(vec![item.clone()]);
        app.open_item.set(Some("pr".into()));
        state.item.set(Some(Arc::new(item)));
        state.pull_request.set(Some(Arc::new(pull)));
        state.ready.set(true);
        state.phase.set(Phase::Ready);
        state.composer_autofocus.set(false);
        state.submit_review_autofocus.set(false);
        let root = TestRoot::new();
        let (backend, commands) = Backend::recording();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        settle(&mut harness);
        (harness, app, state, commands)
    }

    /// Everything the window has asked the backend for since last asked.
    pub(crate) fn sent(
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) -> Vec<Command> {
        let mut sent = Vec::new();
        while let Ok(command) = commands.try_recv() {
            sent.push(command);
        }
        sent
    }
}
