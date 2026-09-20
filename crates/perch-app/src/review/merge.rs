//! Pull-request checks and the deliberate, native merge surface.
//!
//! The overview is a live GitHub snapshot for one exact head.  Cached checks
//! are useful to read while it arrives, but every merge decision below uses
//! the live snapshot and rechecks it again when the confirmation is pressed.

use std::rc::Rc;
use std::sync::Arc;

use floem::event::EventPropagation;
use floem::menu::Menu;
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Effect, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{dyn_container, dyn_stack, Container, Decorators, Empty, Label, Stack};
use floem::AnyView;
use perch_github::{
    Check, CheckRunState, CheckState, Checks, Item, MergeMethod, MergeMethods, MergeState,
    MergeableState, PullRequest, PullRequestOverview, RepositoryPermission, ReviewDecision,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::Checked;
use crate::theme;
use crate::ui::widgets::{checkbox, clickable, open_in_browser, sidebar_glyph};

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
    open: bool,
    draft: bool,
    overview_present: bool,
    overview_loading: bool,
    heads_match: bool,
    has_write_permission: bool,
    mergeable: Option<MergeableState>,
    merge_state: Option<MergeState>,
    can_bypass: bool,
    has_merge_method: bool,
}

/// One restrained surface between the conversation and its composer.
pub fn surface(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let checks_expanded = RwSignal::new(true);
    // GitHub's common default, and the least surprising default when it is
    // enabled. `effective_method` falls back to another repository-enabled
    // method when squash is unavailable.
    let selected_method = RwSignal::new(MergeMethod::Squash);
    let bypass_requirements = RwSignal::new(false);
    let confirmation = RwSignal::new(None::<MergeConfirmation>);

    // Consent and confirmation apply to one exact head, never to whichever
    // pull request happens to occupy this long-lived review view next.
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
            selected_method.set(MergeMethod::Squash);
            bypass_requirements.set(false);
            confirmation.set(None);
        }
        identity
    });
    // A same-head refresh can represent different checks, protection rules,
    // or permissions. Bypass consent is therefore one-shot for the live
    // status generation that presented it, not a sticky preference.
    Effect::new(move |previous: Option<u64>| {
        let generation = state.pull_request_status_generation.get();
        if previous != Some(generation) {
            bypass_requirements.set(false);
            confirmation.set(None);
        }
        generation
    });

    dyn_container(
        move || state.pull_request.with(Option::is_some),
        move |has_pull_request| {
            if !has_pull_request {
                return Empty::new().into_any();
            }

            Container::new(
                Stack::vertical((
                    checks_section(state, checks_expanded),
                    merge_section(
                        state,
                        backend.clone(),
                        selected_method,
                        bypass_requirements,
                        confirmation,
                    ),
                ))
                .style(|s| s.flex_col().width_full()),
            )
            .debug_name("pull-request-checks-and-merge")
            .style(|s| {
                s.width_full()
                    .margin_top(theme::CONVERSATION_ENTRY_PADDING)
                    .background(theme::CANVAS)
                    .border(1.0)
                    .border_color(theme::BORDER)
                    .border_radius(theme::RADIUS_LG)
            })
            .into_any()
        },
    )
}

fn checks_section(state: ReviewState, expanded: RwSignal<bool>) -> impl IntoView {
    let header = Stack::horizontal((
        Label::derived(|| "Checks".to_string()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::FG)
                .selectable(false)
                .pointer_events_none()
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        Label::derived(move || {
            current_checks(state)
                .as_deref()
                .map(checks_summary)
                .unwrap_or_else(|| {
                    if state.pull_request_overview_loading.get() {
                        "Loading…".to_string()
                    } else {
                        "Unavailable".to_string()
                    }
                })
        })
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_SUBTLE)
                .selectable(false)
                .pointer_events_none()
                .margin_right(8.0)
        }),
        dyn_container(
            move || expanded.get(),
            |is_expanded| {
                sidebar_glyph(
                    if is_expanded {
                        SidebarIcon::ChevronUp
                    } else {
                        SidebarIcon::ChevronDown
                    },
                    theme::FG_SUBTLE,
                )
                .style(|s| s.size(12.0, 12.0).pointer_events_none())
            },
        ),
    ))
    .on_event_stop(el::Click, move |_, _| {
        expanded.update(|value| *value = !*value)
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            expanded.update(|value| *value = !*value);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| {
        clickable(s)
            .width_full()
            .height(48.0)
            .items_center()
            .padding_horiz(16.0)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .hover(|s| s.background(theme::CANVAS_SUBTLE))
    });

    let body = dyn_container(
        move || {
            (
                expanded.get(),
                current_checks(state),
                state.pull_request_overview_loading.get(),
            )
        },
        move |(is_expanded, checks, loading)| {
            if !is_expanded {
                return Empty::new().into_any();
            }
            match checks {
                Some(checks) if !checks.runs.is_empty() => checks_rows(checks).into_any(),
                Some(_) => checks_message("No checks reported for this head.").into_any(),
                None if loading => checks_message("Loading checks from GitHub…").into_any(),
                None => checks_message("Checks are unavailable.").into_any(),
            }
        },
    );

    Stack::vertical((header, body)).style(|s| s.flex_col().width_full())
}

fn current_checks(state: ReviewState) -> Option<Arc<Checks>> {
    state
        .pull_request_overview
        .with(|overview| {
            overview
                .as_ref()
                .map(|overview| Arc::new(overview.checks.clone()))
        })
        .or_else(|| state.checks.get())
}

fn checks_rows(checks: Arc<Checks>) -> impl IntoView {
    let rows = Arc::new(checks.runs.iter().cloned().enumerate().collect::<Vec<_>>());
    let row_count = rows.len();
    dyn_stack(
        move || rows.as_ref().clone(),
        |(index, check)| format!("{index}:{}:{:?}", check.name, check.kind),
        move |(index, check)| check_row(check, index + 1 == row_count),
    )
    .style(|s| {
        s.flex_col()
            .width_full()
            .border_top(1.0)
            .border_color(theme::BORDER_MUTED)
    })
}

fn checks_message(message: &'static str) -> impl IntoView {
    Label::derived(move || message.to_string()).style(|s| {
        s.width_full()
            .padding_horiz(16.0)
            .padding_vert(14.0)
            .border_top(1.0)
            .border_color(theme::BORDER_MUTED)
            .font_size(theme::TEXT_META)
            .color(theme::FG_SUBTLE)
    })
}

fn check_row(check: Check, last: bool) -> impl IntoView {
    let (icon, icon_color) = check_icon(check.state);
    let debug = format!("check-row:{}", check.name);
    let name = check_label(&check);
    let status = check_status_label(&check);
    let status_color = check_color(check.state);
    let url = check.url.filter(|url| !url.is_empty());
    let click_url = url.clone();
    let key_url = url.clone();
    let interactive = url.is_some();

    Stack::horizontal((
        sidebar_glyph(icon, icon_color)
            .style(|s| s.size(15.0, 15.0).margin_right(10.0).pointer_events_none()),
        Label::derived(move || name.clone()).style(|s| {
            s.min_width(0.0)
                .flex_grow(1.0_f32)
                .font_size(theme::TEXT_META)
                .color(theme::FG)
                .text_ellipsis()
                .selectable(false)
                .pointer_events_none()
        }),
        Label::derived(move || status.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(status_color)
                .margin_left(16.0)
                .selectable(false)
                .pointer_events_none()
        }),
    ))
    .on_event_stop(el::Click, move |_, _| {
        if let Some(url) = click_url.as_deref() {
            open_in_browser(url);
        }
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            if let Some(url) = key_url.as_deref() {
                open_in_browser(url);
                return EventPropagation::Stop;
            }
        }
        EventPropagation::Continue
    })
    .style(move |s| {
        s.width_full()
            .height(38.0)
            .items_center()
            .padding_horiz(16.0)
            .apply_if(!last, |s| {
                s.border_bottom(1.0).border_color(theme::BORDER_MUTED)
            })
            .apply_if(interactive, |s| {
                clickable(s)
                    .keyboard_navigable()
                    .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
                    .hover(|s| s.background(theme::CANVAS_SUBTLE))
            })
    })
    .debug_name(debug)
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

fn check_label(check: &Check) -> String {
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

fn check_status_label(check: &Check) -> String {
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

fn checks_summary(checks: &Checks) -> String {
    if checks.runs.is_empty() {
        return match checks.state {
            CheckState::Passing => "Checks passed",
            CheckState::Failing { .. } => "Checks failed",
            CheckState::Running => "Checks running",
            CheckState::Unknown => "No checks",
        }
        .to_string();
    }
    let failures = checks
        .runs
        .iter()
        .filter(|check| check.state.is_failing())
        .count();
    let pending = checks
        .runs
        .iter()
        .filter(|check| check.state.is_pending())
        .count();
    let passing = checks
        .runs
        .iter()
        .filter(|check| check.state.is_passing())
        .count();
    let unknown = checks
        .runs
        .len()
        .saturating_sub(failures + pending + passing);

    if failures > 0 {
        return format!("{} failed · {} passed", failures, passing);
    }
    if matches!(checks.state, CheckState::Failing { .. }) {
        return "Checks failed".to_string();
    }
    if pending > 0 {
        return format!("{} running · {} passed", pending, passing);
    }
    if matches!(checks.state, CheckState::Running) {
        return format!("Checks running · {} passed", passing);
    }
    if unknown > 0 {
        return format!("{} unknown · {} passed", unknown, passing);
    }
    format!("{} checks passed", passing)
}

fn check_icon(state: CheckRunState) -> (SidebarIcon, Color) {
    if state.is_passing() {
        (SidebarIcon::Check, theme::OPEN)
    } else if state.is_failing() {
        (SidebarIcon::Close, theme::CLOSED)
    } else if state.is_pending() {
        (SidebarIcon::Refresh, theme::DRAFT_YELLOW)
    } else {
        (SidebarIcon::Warning, theme::FG_SUBTLE)
    }
}

fn check_color(state: CheckRunState) -> Color {
    if state.is_passing() {
        theme::OPEN
    } else if state.is_failing() {
        theme::CLOSED
    } else if state.is_pending() {
        theme::DRAFT_YELLOW
    } else {
        theme::FG_SUBTLE
    }
}

fn check_status(state: CheckRunState) -> &'static str {
    match state {
        CheckRunState::Pending => "In progress",
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

fn merge_section(
    state: ReviewState,
    backend: Rc<Backend>,
    selected_method: RwSignal<MergeMethod>,
    bypass_requirements: RwSignal<bool>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
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
        },
        move |snapshot| {
            merge_content(
                state,
                backend.clone(),
                selected_method,
                bypass_requirements,
                confirmation,
                snapshot,
            )
        },
    )
}

fn merge_content(
    state: ReviewState,
    backend: Rc<Backend>,
    selected_method: RwSignal<MergeMethod>,
    bypass_requirements: RwSignal<bool>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
    snapshot: MergeSnapshot,
) -> AnyView {
    let (Some(pull), Some(item)) = (snapshot.pull.as_ref(), snapshot.item.as_ref()) else {
        return Empty::new().into_any();
    };
    let decision = merge_decision(&snapshot, pull, item);
    let status = merge_status(decision, snapshot.overview.as_deref());
    let controls = merge_controls(
        state,
        backend,
        selected_method,
        bypass_requirements,
        confirmation,
        decision,
        snapshot.clone(),
        pull.clone(),
        item.clone(),
    );

    Stack::vertical((
        status,
        error_line(snapshot.overview_error.clone()),
        error_line(snapshot.merge_error.clone()),
        controls,
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .gap(10.0)
            .padding(16.0)
            .border_top(1.0)
            .border_color(theme::BORDER)
    })
    .into_any()
}

fn merge_status(decision: MergeDecision, overview: Option<&PullRequestOverview>) -> impl IntoView {
    let (icon, color, title, detail) = match decision {
        MergeDecision::Loading => (
            SidebarIcon::Refresh,
            theme::DRAFT_YELLOW,
            "Checking merge requirements".to_string(),
            "Fetching a fresh status from GitHub.".to_string(),
        ),
        MergeDecision::Conflicts => (
            SidebarIcon::Warning,
            theme::DRAFT,
            "This branch has conflicts that must be resolved".to_string(),
            "Resolve the conflicts on GitHub before continuing.".to_string(),
        ),
        MergeDecision::Ready => (
            SidebarIcon::Check,
            theme::OPEN,
            "Ready to merge".to_string(),
            ready_detail(overview),
        ),
        MergeDecision::Bypass => (
            SidebarIcon::Warning,
            theme::CLOSED,
            "Merging is blocked".to_string(),
            blocker_detail(overview),
        ),
        MergeDecision::Disabled(reason) => {
            let (title, detail) = disabled_copy(reason, overview);
            (SidebarIcon::Warning, theme::FG_SUBTLE, title, detail)
        }
    };

    Stack::horizontal((
        Container::new(sidebar_glyph(icon, color).style(|s| s.size(16.0, 16.0))).style(|s| {
            s.size(30.0, 30.0)
                .items_center()
                .justify_center()
                .background(theme::CANVAS_SUBTLE)
                .border_radius(15.0)
                .margin_right(10.0)
        }),
        Stack::vertical((
            Label::derived(move || title.clone()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG)
            }),
            Label::derived(move || detail.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_MUTED)
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
    selected_method: RwSignal<MergeMethod>,
    bypass_requirements: RwSignal<bool>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
    decision: MergeDecision,
    snapshot: MergeSnapshot,
    pull: Arc<PullRequest>,
    item: Arc<Item>,
) -> AnyView {
    match decision {
        MergeDecision::Conflicts => {
            let url = conflicts_url(&pull.html_url);
            let open_url = url.clone();
            action_button(
                "Resolve conflicts on GitHub".to_string(),
                false,
                move || url.is_some(),
                move || {
                    if let Some(url) = open_url.as_deref() {
                        open_in_browser(url);
                    }
                },
            )
            .into_any()
        }
        MergeDecision::Loading => {
            action_button("Checking status…".to_string(), false, || false, || {}).into_any()
        }
        MergeDecision::Disabled(_) => {
            action_button("Merge unavailable".to_string(), false, || false, || {}).into_any()
        }
        MergeDecision::Ready | MergeDecision::Bypass => {
            let Some(overview) = snapshot.overview.as_ref() else {
                return Empty::new().into_any();
            };
            if snapshot.merge_pending {
                return action_button("Merging…".to_string(), true, || false, || {}).into_any();
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
                return confirmation_controls(state, backend, confirmation, request, pull)
                    .into_any();
            }

            let bypass_control = if bypass_needed {
                bypass_checkbox(bypass_requirements, confirmation).into_any()
            } else {
                Empty::new().into_any()
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
            let start_enabled = !bypass_needed || bypass;
            let action_label = if bypass_needed {
                format!("Bypass rules and merge ({})", method_short(method))
            } else {
                method_label(method).to_string()
            };
            let action = action_button(
                action_label,
                true,
                move || start_enabled,
                move || confirmation.set(Some(start_request.clone())),
            )
            .style(|s| s.flex_grow(1.0_f32));
            let method_picker = merge_method_button(
                overview.allowed_merge_methods,
                selected_method,
                confirmation,
            );

            Stack::vertical((
                bypass_control,
                Stack::horizontal((action, method_picker))
                    .style(|s| s.width_full().items_center().gap(8.0)),
            ))
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
                    .color(theme::CLOSED)
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
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
    })
    .debug_name("merge-bypass")
}

fn confirmation_controls(
    state: ReviewState,
    backend: Rc<Backend>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
    request: MergeConfirmation,
    pull: Arc<PullRequest>,
) -> impl IntoView {
    let summary = format!(
        "Merge {} into {} using {}?",
        short_sha(&request.head_sha),
        pull.base_ref,
        method_short(request.method),
    );
    let cancel = action_button(
        "Cancel".to_string(),
        false,
        || true,
        move || confirmation.set(None),
    );

    let enabled_request = request.clone();
    let submit_request = request.clone();
    let submit = action_button(
        if request.bypass_requirements {
            "Confirm bypass and merge".to_string()
        } else {
            "Confirm merge".to_string()
        },
        true,
        move || request_is_current(state, &enabled_request),
        move || {
            if !request_is_current(state, &submit_request) {
                confirmation.set(None);
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
            );
        },
    );

    Stack::vertical((
        Label::derived(move || summary.clone())
            .style(|s| s.font_size(theme::TEXT_META).color(theme::FG_MUTED)),
        Stack::horizontal((Empty::new().style(|s| s.flex_grow(1.0_f32)), cancel, submit))
            .style(|s| s.width_full().items_center().gap(8.0)),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .gap(10.0)
            .padding(12.0)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::RADIUS)
    })
}

fn merge_method_button(
    methods: MergeMethods,
    selected_method: RwSignal<MergeMethod>,
    confirmation: RwSignal<Option<MergeConfirmation>>,
) -> impl View {
    Stack::horizontal((
        Label::derived(move || {
            effective_method(selected_method.get(), methods)
                .map(method_short)
                .unwrap_or("Method")
                .to_string()
        })
        .style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::FG)
                .selectable(false)
                .pointer_events_none()
                .margin_right(5.0)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE)
            .style(|s| s.size(9.0, 9.0).pointer_events_none()),
    ))
    .popout_menu(move || merge_method_menu(methods, selected_method, confirmation))
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            floem::action::show_context_menu(
                merge_method_menu(methods, selected_method, confirmation),
                None,
            );
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| {
        clickable(s)
            .items_center()
            .height(30.0)
            .padding_horiz(9.0)
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
    .debug_name("merge-method")
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

fn action_button(
    label: String,
    prominent: bool,
    enabled: impl Fn() -> bool + 'static,
    action: impl Fn() + 'static,
) -> impl View {
    let debug = format!("merge-action:{}", label.replace(' ', "-"));
    let enabled: Rc<dyn Fn() -> bool> = Rc::new(enabled);
    let event_enabled = enabled.clone();
    let style_enabled = enabled.clone();
    let action: Rc<dyn Fn()> = Rc::new(action);
    let click = action.clone();
    Container::new(Label::derived(move || label.clone()).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(FontWeight::SEMI_BOLD)
            .color(if prominent {
                theme::CANVAS
            } else {
                theme::FG_MUTED
            })
            .selectable(false)
            .pointer_events_none()
    }))
    .on_event_stop(el::Click, move |_, _| {
        if event_enabled() {
            click();
        }
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) && enabled() {
            action();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(move |s| {
        let available = style_enabled();
        clickable(s)
            .height(30.0)
            .padding_horiz(11.0)
            .items_center()
            .justify_center()
            .background(if prominent { theme::FG } else { theme::CANVAS })
            .border(1.0)
            .border_color(if prominent { theme::FG } else { theme::BORDER })
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .set_disabled(!available)
            .apply_if(!available, |s| {
                s.opacity(0.45_f32).cursor(CursorStyle::Default)
            })
            .apply_if(available, |s| {
                s.hover(move |s| {
                    s.background(if prominent {
                        theme::ACTION_HOVER
                    } else {
                        theme::CANVAS_HOVER
                    })
                })
            })
    })
    .debug_name(debug)
    .into_view()
}

fn error_line(message: Option<String>) -> AnyView {
    match message {
        Some(message) => Stack::horizontal((
            sidebar_glyph(SidebarIcon::Warning, theme::CLOSED)
                .style(|s| s.size(14.0, 14.0).margin_right(7.0)),
            Label::derived(move || message.clone()).style(|s| {
                s.min_width(0.0)
                    .flex_grow(1.0_f32)
                    .font_size(theme::TEXT_META)
                    .color(theme::CLOSED)
            }),
        ))
        .style(|s| s.width_full().items_start())
        .into_any(),
        None => Empty::new().into_any(),
    }
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
    MergeFacts {
        offline,
        historical,
        stale,
        open: pull.is_open() && !pull.merged,
        draft: pull.draft,
        overview_present: overview.is_some(),
        overview_loading,
        heads_match,
        has_write_permission: overview.is_some_and(|overview| {
            overview.viewer_can_merge_as_admin
                || matches!(
                    overview.viewer_permission,
                    Some(
                        RepositoryPermission::Admin
                            | RepositoryPermission::Maintain
                            | RepositoryPermission::Write
                    )
                )
        }),
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
    if facts.stale {
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
    if !facts.heads_match {
        return MergeDecision::Disabled(DisabledReason::StaleCheckout);
    }
    if !facts.has_write_permission {
        return MergeDecision::Disabled(DisabledReason::NoWritePermission);
    }
    if matches!(facts.mergeable, Some(MergeableState::Conflicting))
        || matches!(facts.merge_state, Some(MergeState::Dirty))
    {
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
) -> (String, String) {
    let title = "Merging is unavailable".to_string();
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

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
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
            open: true,
            draft: false,
            overview_present: true,
            overview_loading: false,
            heads_match: true,
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
            })));
        state
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
    fn checks_summary_never_calls_unknown_or_truncated_details_all_passed() {
        let empty_pass = Checks {
            state: CheckState::Passing,
            url: None,
            runs: Vec::new(),
        };
        assert_eq!(checks_summary(&empty_pass), "Checks passed");

        let partial = Checks {
            state: CheckState::Passing,
            url: None,
            runs: vec![check(CheckRunState::Success), check(CheckRunState::Unknown)],
        };
        assert_eq!(checks_summary(&partial), "1 unknown · 1 passed");

        let truncated_failure = Checks {
            state: CheckState::Failing {
                workflow: "a check outside the returned details".into(),
            },
            url: None,
            runs: vec![check(CheckRunState::Success)],
        };
        assert_eq!(checks_summary(&truncated_failure), "Checks failed");
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
    fn rendered_surface_shows_checks_and_supports_keyboard_bypass_confirmation() {
        let root = TestRoot::new();
        let state = review_state(MergeState::Blocked, true);
        let view = surface(state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        assert!(named(harness.root_id(), "check-row:check").is_some());
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
}
