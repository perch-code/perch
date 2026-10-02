//! Agent activity: each item's runs in its Agents section on Conversation
//! (`section`), every item's in the dropdown from the bar's Agents button
//! (`dropdown`), what a run did as its row opens to it (`detail`), and the
//! toast a run leaves as it finishes.

pub mod model;

mod detail;
pub mod dropdown;
mod review_row;
pub mod section;

use std::rc::Rc;
use std::time::Duration;

use floem::event::EventPropagation;
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Effect, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Container, Decorators, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, sidebar_glyph};

use model::{AgentRunStatus, AgentRunSummary};

/// The clock that keeps the elapsed times moving runs while there is work
/// to time. Once, from the window.
pub fn wire(state: AppState) {
    Effect::new(move |_| {
        let active = state
            .agent_runs
            .with(|runs| runs.iter().any(|run| run.status.is_active()));
        if active && !state.agents_clock_armed.get_untracked() {
            arm_clock(state);
        }
    });
}

pub(super) fn request_agent_runs(state: AppState, backend: &Backend, history_limit: usize) {
    let token = state.begin_agent_history_load();
    backend.load_agent_runs(history_limit);
    floem::action::exec_after(Duration::from_secs(5), move |_| {
        state.time_out_agent_history_load(token);
    });
}

pub(crate) fn icon_action(
    icon: SidebarIcon,
    debug: &'static str,
    action: impl Fn() + 'static,
) -> impl View {
    let action: Rc<dyn Fn()> = Rc::new(action);
    let click = action.clone();
    Container::new(
        sidebar_glyph(icon, theme::fg_muted).style(|s| s.size(14.0, 14.0).pointer_events_none()),
    )
    .on_event_stop(el::Click, move |_, _| click())
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            action();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| {
        clickable(s)
            .size(28.0, 28.0)
            .items_center()
            .justify_center()
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .hover(|s| s.background(theme::canvas_hover()))
    })
    .debug_name(debug)
    .into_view()
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
        AgentRunStatus::Running => theme::accent,
        AgentRunStatus::Completed => theme::open,
        AgentRunStatus::Failed => theme::closed,
        AgentRunStatus::Stopped => theme::fg_subtle,
        AgentRunStatus::Queued
        | AgentRunStatus::Starting
        | AgentRunStatus::Stopping
        | AgentRunStatus::Interrupted => theme::draft_yellow,
    }
}

pub(super) fn metadata(run: &AgentRunSummary) -> String {
    let model = crate::review::threads::model_label(&run.model);
    let effort = perch_agent::Effort::parse(&run.effort)
        .map(|effort| effort.label().to_string())
        .unwrap_or_else(|| run.effort.clone());
    let work = match run.mode {
        perch_github::RunMode::Work => "Work · ",
        perch_github::RunMode::Ask => "",
    };
    format!("{work}{} · {model} · {effort}", run.backend_label())
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
    let dismiss = icon_action(SidebarIcon::Close, "Dismiss agent completion", move || {
        state.dismiss_toast(token)
    });
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

        app.merge_agent_runs([perch_github::RunSummary::from(&running_record())], false);
        harness.rebuild();
        assert!(app.agents_clock_armed.get());
    }

}
