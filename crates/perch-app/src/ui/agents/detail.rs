//! One run's request, activity and result.

use std::rc::Rc;
use std::sync::Arc;

use floem::peniko::kurbo::Point;
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::text::FontWeight;
use floem::views::scroll::ScrollChanged;
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};
use perch_github::comments::{RunEvent, RunEventRecord};

use crate::backend::Backend;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;

use super::model::{elapsed_label, AgentRunStatus, AgentRunSummary};
use super::{metadata, navigate_to_source, status_color, text_action};

pub fn view(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    initial: AgentRunSummary,
) -> impl IntoView {
    let id = Arc::new(initial.id.clone());
    let initial = Arc::new(initial);
    let header_id = id.clone();
    let header_initial = initial.clone();
    let summary_backend = backend.clone();
    Stack::vertical((
        dyn_container(
            move || current_run(state, &header_id, &header_initial),
            move |run| summary_bar(state, review, summary_backend.clone(), run).into_any(),
        ),
        detail_body(state, backend, id, initial),
    ))
    .style(|s| s.size_full().flex_col().min_height(0.0))
    .debug_name("agent-run-detail")
}

fn current_run(state: AppState, id: &str, initial: &AgentRunSummary) -> AgentRunSummary {
    state.agent_run(id).unwrap_or_else(|| initial.clone())
}

fn current_events(state: AppState, id: &str) -> Arc<Vec<RunEventRecord>> {
    state
        .agent_run_events
        .with(|all| all.get(id).cloned())
        .unwrap_or_else(|| Arc::new(Vec::new()))
}

fn detail_body(
    state: AppState,
    backend: Rc<Backend>,
    id: Arc<String>,
    initial: Arc<AgentRunSummary>,
) -> impl IntoView {
    let activity_id = id.clone();
    let activity_initial = initial.clone();
    let result_id = id.clone();
    let result_initial = initial.clone();
    let usage_id = id.clone();
    let usage_initial = initial.clone();
    let content = Stack::vertical((
        request_section(initial.request.clone()),
        activity_section(state, backend, activity_id, activity_initial),
        result_section(state, result_id, result_initial),
        usage_section(state, usage_id, usage_initial),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .gap(22.0)
            .padding(16.0)
            .padding_bottom(28.0)
    });
    let content_id = content.view_id();
    let follow = RwSignal::new(true);
    let revision_id = id.clone();
    let revision_initial = initial.clone();
    let scroller = Container::new(content)
        .style(|s| s.width_full())
        .scroll()
        .scroll_to(move || {
            let run = current_run(state, &revision_id, &revision_initial);
            let event_count = current_events(state, &revision_id).len();
            let _revision = (
                run.last_event_sequence,
                run.status,
                run.response.len(),
                event_count,
            );
            follow.get().then_some(Point::new(0.0, f64::MAX))
        });
    let scroll_id = scroller.view_id();
    let scroller = scroller
        .on_event_stop(
            ScrollChanged::listener(),
            move |_, event: &ScrollChanged| {
                let viewport = scroll_id.get_content_rect_local().height();
                let content = content_id.get_layout_rect_local().height();
                let at_bottom = event.offset.y + viewport >= content - 12.0;
                if follow.get_untracked() != at_bottom {
                    follow.set(at_bottom);
                }
            },
        )
        .style(|s| {
            s.size_full()
                .min_height(0.0)
                .width_full()
                .background(theme::CANVAS_SUBTLE)
        });
    let jump = dyn_container(
        move || !follow.get(),
        move |show| {
            if show {
                text_action(
                    || "Jump to latest".to_string(),
                    true,
                    move || follow.set(true),
                )
                .into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| {
        s.position(Position::Absolute)
            .inset_right(14.0)
            .inset_bottom(12.0)
            .z_index(3)
    });
    Stack::new((scroller, jump)).style(|s| {
        s.flex_grow(1.0_f32)
            .min_height(0.0)
            .width_full()
            .position(Position::Relative)
    })
}

fn summary_bar(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    run: AgentRunSummary,
) -> impl IntoView {
    let color = status_color(run.status);
    let meta = metadata(&run);
    let elapsed_run = run.clone();
    let source_available = run.source_available
        && run
            .item_id
            .as_deref()
            .is_some_and(|item_id| state.item(item_id).is_some());
    let view = if source_available {
        let view_run = run.clone();
        let view_backend = backend.clone();
        text_action(
            || "View discussion".to_string(),
            true,
            move || navigate_to_source(state, review, &view_backend, &view_run),
        )
        .into_any()
    } else {
        Label::derived(|| "Source unavailable".to_string())
            .style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::FG_SUBTLE)
                    .padding_horiz(8.0)
                    .selectable(false)
            })
            .into_any()
    };
    let control = match run.status {
        AgentRunStatus::Queued => {
            let id = run.id.clone();
            text_action(
                || "Cancel".to_string(),
                false,
                move || backend.cancel_run(&id),
            )
            .into_any()
        }
        status if status.may_stop() => {
            let id = run.id.clone();
            text_action(
                || "Stop".to_string(),
                false,
                move || backend.cancel_run(&id),
            )
            .into_any()
        }
        status if status.may_retry() => {
            let id = run.id.clone();
            text_action(
                || "Retry".to_string(),
                false,
                move || backend.retry_run(&id),
            )
            .into_any()
        }
        AgentRunStatus::Stopping => Label::derived(|| "Stopping…".to_string())
            .style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::FG_SUBTLE)
                    .padding_horiz(8.0)
                    .selectable(false)
            })
            .into_any(),
        _ => Empty::new().into_any(),
    };

    Stack::vertical((
        Stack::horizontal((
            Empty::new().style(move |s| {
                s.size(8.0, 8.0)
                    .border_radius(4.0)
                    .background(color)
                    .flex_shrink(0.0_f32)
            }),
            Label::derived(move || meta.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG)
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
            }),
            Label::derived(move || {
                format!(
                    "{} · {}",
                    elapsed_run.status.label(),
                    elapsed_label(&elapsed_run, state.now.get())
                )
            })
            .style(move |s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(FontWeight::MEDIUM)
                    .color(color)
                    .flex_shrink(0.0_f32)
                    .selectable(false)
            }),
        ))
        .style(|s| s.items_center().width_full().gap(7.0)),
        Stack::horizontal((
            Label::derived(move || {
                let group = run.group_label();
                if group.is_empty() {
                    run.purpose.label().to_string()
                } else {
                    format!("{} · {group}", run.purpose.label())
                }
            })
            .style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::FG_SUBTLE)
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
            }),
            view,
            control,
        ))
        .style(|s| s.items_center().width_full().gap(3.0)),
    ))
    .style(|s| {
        s.width_full()
            .flex_col()
            .gap(7.0)
            .padding_left(14.0)
            .padding_right(10.0)
            .padding_vert(10.0)
            .background(theme::CANVAS)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

fn heading(text: &'static str) -> impl IntoView {
    Label::derived(move || text.to_string()).style(|s| {
        s.font_size(theme::TEXT_SECTION)
            .font_weight(FontWeight::SEMI_BOLD)
            .color(theme::FG_SUBTLE)
            .selectable(false)
    })
}

fn request_section(request: String) -> impl IntoView {
    Stack::vertical((
        heading("REQUEST"),
        Container::new(Label::derived(move || request.clone()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .color(theme::FG)
                .line_height(1.45)
                .text_wrap()
                .width_full()
        }))
        .style(move |s| {
            s.width_full()
                .padding_left(12.0)
                .padding_vert(3.0)
                .border_left(2.0)
                .border_color(theme::ACCENT)
        }),
    ))
    .style(|s| s.flex_col().width_full().gap(9.0))
}

fn activity_section(
    state: AppState,
    backend: Rc<Backend>,
    id: Arc<String>,
    initial: Arc<AgentRunSummary>,
) -> impl IntoView {
    let rows_id = id.clone();
    let rows_initial = initial.clone();
    let page_state_id = id.clone();
    let page_action_id = id.clone();
    let empty_id = id;
    let empty_initial = initial;
    Stack::vertical((
        heading("ACTIVITY"),
        dyn_container(
            move || {
                let (has_more, loading) = state.agent_run_events_page_state(&page_state_id);
                let before = current_events(state, &page_state_id)
                    .first()
                    .map(|event| event.sequence);
                (has_more, loading, before)
            },
            move |(has_more, loading, before)| {
                if !has_more {
                    return Empty::new().into_any();
                }
                let load_backend = backend.clone();
                let run_id = page_action_id.clone();
                text_action(
                    move || {
                        if loading {
                            "Loading earlier activity…".to_string()
                        } else {
                            "Load earlier activity".to_string()
                        }
                    },
                    false,
                    move || {
                        if !loading {
                            if let Some(before) = before {
                                state.set_agent_run_events_loading(&run_id);
                                load_backend.load_agent_run_events(&run_id, before);
                            }
                        }
                    },
                )
                .into_any()
            },
        ),
        dyn_stack(
            move || {
                let run = current_run(state, &rows_id, &rows_initial);
                activity_rows(&run, &current_events(state, &rows_id))
            },
            |row| (row.sequence, row.text.clone(), row.ok),
            |row| {
                Stack::horizontal((
                    Empty::new().style(move |s| {
                        s.size(6.0, 6.0)
                            .border_radius(3.0)
                            .background(if row.ok {
                                theme::FG_SUBTLE
                            } else {
                                theme::CLOSED
                            })
                            .margin_top(6.0)
                            .flex_shrink(0.0_f32)
                    }),
                    Label::derived(move || row.text.clone()).style(move |s| {
                        s.font_size(theme::TEXT_BADGE)
                            .font_family(theme::CODE_FONT_STACK.to_string())
                            .color(if row.ok {
                                theme::FG_MUTED
                            } else {
                                theme::CLOSED
                            })
                            .line_height(1.4)
                            .text_wrap()
                            .min_width(0.0)
                            .flex_grow(1.0_f32)
                    }),
                ))
                .style(|s| s.items_start().width_full().gap(9.0).padding_vert(2.0))
            },
        )
        .style(|s| s.flex_col().width_full()),
        dyn_container(
            move || {
                let run = current_run(state, &empty_id, &empty_initial);
                let empty = activity_rows(&run, &current_events(state, &empty_id)).is_empty();
                let loading = state.agent_run_events_page_state(&empty_id).1;
                (empty, loading)
            },
            |(empty, loading)| {
                if empty {
                    Label::derived(move || {
                        if loading {
                            "Loading activity…".to_string()
                        } else {
                            "No tool activity reported.".to_string()
                        }
                    })
                    .style(|s| s.font_size(theme::TEXT_META).color(theme::FG_SUBTLE))
                    .into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        ),
    ))
    .style(|s| s.flex_col().width_full().gap(9.0))
}

#[derive(Clone, PartialEq, Eq)]
struct ActivityRow {
    sequence: u64,
    text: String,
    ok: bool,
}

fn activity_rows(run: &AgentRunSummary, events: &[RunEventRecord]) -> Vec<ActivityRow> {
    if events.is_empty() {
        return run
            .steps
            .iter()
            .map(|step| ActivityRow {
                sequence: step.sequence,
                text: step.summary.clone(),
                ok: step.ok,
            })
            .collect();
    }
    events
        .iter()
        .filter_map(|record| {
            let (text, ok) = match &record.event {
                RunEvent::State { status } => (status.name().to_string(), true),
                RunEvent::Started { model, .. } => (
                    model
                        .as_ref()
                        .map(|model| format!("Started {model}"))
                        .unwrap_or_else(|| "Started".into()),
                    true,
                ),
                RunEvent::ToolCall {
                    summary, parent_id, ..
                } => (
                    if parent_id.is_some() {
                        format!("↳ {summary}")
                    } else {
                        summary.clone()
                    },
                    true,
                ),
                RunEvent::ToolResult {
                    ok: false, summary, ..
                } => (
                    if summary.is_empty() {
                        "Tool call was not allowed".into()
                    } else {
                        summary.clone()
                    },
                    false,
                ),
                RunEvent::ToolResult {
                    ok: true, summary, ..
                } => (
                    if summary.is_empty() {
                        "Tool call completed".into()
                    } else {
                        summary.clone()
                    },
                    true,
                ),
                RunEvent::PermissionDenied { tool, message } => (
                    tool.as_deref()
                        .map(|tool| format!("{tool}: {message}"))
                        .unwrap_or_else(|| message.clone()),
                    false,
                ),
                RunEvent::Completed { .. } => ("Completed".into(), true),
                RunEvent::Terminated { message, .. } => (message.clone(), false),
                RunEvent::LegacyStep { summary, ok } => (summary.clone(), *ok),
                RunEvent::Text { .. } | RunEvent::Usage { .. } => return None,
            };
            Some(ActivityRow {
                sequence: record.sequence,
                text,
                ok,
            })
        })
        .collect()
}

fn result_section(
    state: AppState,
    id: Arc<String>,
    initial: Arc<AgentRunSummary>,
) -> impl IntoView {
    let events_id = id.clone();
    let page_state_id = id.clone();
    dyn_container(
        move || {
            (
                current_run(state, &id, &initial),
                current_events(state, &events_id),
                state.agent_run_events_page_state(&page_state_id).0,
            )
        },
        |(run, events, has_more)| result_content(run, events, has_more).into_any(),
    )
}

fn response_text(
    run: &AgentRunSummary,
    events: &[RunEventRecord],
    has_more: bool,
) -> (String, Option<&'static str>) {
    let streamed = events
        .iter()
        .filter_map(|record| match &record.event {
            RunEvent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if run.response_truncated {
        if streamed.trim().is_empty() {
            return (
                run.response.clone(),
                Some("The stored response is truncated; response activity has not been fully loaded."),
            );
        }
        if has_more {
            return (
                streamed,
                Some("This response is incomplete. Load earlier activity to reconstruct the missing beginning."),
            );
        }
        if events
            .iter()
            .any(|record| record.truncated && matches!(&record.event, RunEvent::Text { .. }))
        {
            return (
                streamed,
                Some("Some response activity was truncated in storage."),
            );
        }
        return (streamed, None);
    }
    if run.response.trim().is_empty() {
        (streamed, None)
    } else {
        (run.response.clone(), None)
    }
}

fn result_content(
    run: AgentRunSummary,
    events: Arc<Vec<RunEventRecord>>,
    has_more: bool,
) -> impl IntoView {
    let (result, incomplete) = response_text(&run, &events, has_more);
    let error = (!matches!(run.status, AgentRunStatus::Completed))
        .then(|| run.terminal_display())
        .flatten();
    let outcome_color = status_color(run.status);
    let result_is_empty = result.trim().is_empty();
    let status = run.status;
    let content = match (
        error.filter(|_| status != AgentRunStatus::Completed),
        result_is_empty,
    ) {
        (Some(error), false) => Stack::vertical((
            Container::new(Label::derived(move || error.clone()).style(move |s| {
                s.font_size(theme::TEXT_BODY)
                    .color(outcome_color)
                    .line_height(1.45)
                    .text_wrap()
                    .width_full()
            }))
            .style(move |s| {
                s.width_full()
                    .padding(11.0)
                    .background(outcome_color.with_alpha(0.07))
                    .border(1.0)
                    .border_color(outcome_color.with_alpha(0.30))
                    .border_radius(theme::RADIUS)
            }),
            crate::review::markdown::render(&result),
        ))
        .style(|s| s.flex_col().width_full().gap(12.0))
        .into_any(),
        (Some(error), true) => {
            Container::new(Label::derived(move || error.clone()).style(move |s| {
                s.font_size(theme::TEXT_BODY)
                    .color(outcome_color)
                    .line_height(1.45)
                    .text_wrap()
                    .width_full()
            }))
            .style(move |s| {
                s.width_full()
                    .padding(11.0)
                    .background(outcome_color.with_alpha(0.07))
                    .border(1.0)
                    .border_color(outcome_color.with_alpha(0.30))
                    .border_radius(theme::RADIUS)
            })
            .into_any()
        }
        (None, true) => Label::derived(move || {
            if status.is_active() {
                "Waiting for the agent’s response…".to_string()
            } else {
                "No response was reported.".to_string()
            }
        })
        .style(|s| s.font_size(theme::TEXT_META).color(theme::FG_SUBTLE))
        .into_any(),
        (None, false) => crate::review::markdown::render(&result).into_any(),
    };
    let content = if let Some(message) = incomplete {
        Stack::vertical((
            Container::new(Label::derived(move || message.to_string()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .line_height(1.4)
                    .text_wrap()
                    .width_full()
            }))
            .style(|s| {
                s.width_full()
                    .padding(9.0)
                    .background(theme::BADGE_BG)
                    .border_radius(theme::RADIUS)
            }),
            content,
        ))
        .style(|s| s.flex_col().width_full().gap(10.0))
        .into_any()
    } else {
        content
    };
    Stack::vertical((
        heading(if run.status == AgentRunStatus::Failed {
            "ERROR"
        } else {
            "RESPONSE"
        }),
        content,
    ))
    .style(|s| s.flex_col().width_full().gap(9.0))
}

fn usage_section(state: AppState, id: Arc<String>, initial: Arc<AgentRunSummary>) -> impl IntoView {
    dyn_container(
        move || current_run(state, &id, &initial),
        |run| usage_content(run).into_any(),
    )
}

fn usage_content(run: AgentRunSummary) -> impl IntoView {
    let usage = usage_label(&run);
    Stack::vertical((
        heading("USAGE"),
        Label::derived(move || usage.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::FG_SUBTLE)
                .text_wrap()
                .width_full()
        }),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .gap(7.0)
            .padding_top(14.0)
            .border_top(1.0)
            .border_color(theme::BORDER_MUTED)
    })
}

fn usage_label(run: &AgentRunSummary) -> String {
    if run.input_tokens.is_none()
        && run.output_tokens.is_none()
        && run.cached_input_tokens.is_none()
        && run.cost_usd.is_none()
    {
        return "Usage unavailable".to_string();
    }
    [
        run.input_tokens
            .map(|tokens| format!("{tokens} input tokens"))
            .unwrap_or_else(|| "Input tokens unavailable".into()),
        run.output_tokens
            .map(|tokens| format!("{tokens} output tokens"))
            .unwrap_or_else(|| "Output tokens unavailable".into()),
        run.cached_input_tokens
            .map(|tokens| format!("{tokens} cached input tokens"))
            .unwrap_or_else(|| "Cached input tokens unavailable".into()),
        run.cost_usd
            .map(|cost| format!("${cost:.2}"))
            .unwrap_or_else(|| "Cost unavailable".into()),
    ]
    .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> AgentRunSummary {
        let run = perch_github::RunRecord {
            id: "run".into(),
            backend: "codex".into(),
            model: "gpt".into(),
            effort: "medium".into(),
            queued_at: jiff::Timestamp::UNIX_EPOCH,
            ..perch_github::RunRecord::default()
        };
        AgentRunSummary::from_record(&run)
    }

    fn event(sequence: u64, event: RunEvent) -> RunEventRecord {
        RunEventRecord {
            run_id: "run".into(),
            sequence,
            event_id: format!("event-{sequence}"),
            occurred_at: jiff::Timestamp::UNIX_EPOCH,
            event,
            truncated: false,
        }
    }

    #[test]
    fn activity_keeps_nested_successes_and_permission_denials() {
        let rows = activity_rows(
            &summary(),
            &[
                event(
                    1,
                    RunEvent::ToolCall {
                        id: "child".into(),
                        name: "Task".into(),
                        summary: "Inspect parser".into(),
                        parent_id: Some("parent".into()),
                    },
                ),
                event(
                    2,
                    RunEvent::ToolResult {
                        id: "child".into(),
                        ok: true,
                        summary: "Parser inspected".into(),
                    },
                ),
                event(
                    3,
                    RunEvent::PermissionDenied {
                        tool: Some("Read".into()),
                        message: "outside the checkout".into(),
                    },
                ),
            ],
        );
        assert_eq!(rows[0].text, "↳ Inspect parser");
        assert_eq!(rows[1].text, "Parser inspected");
        assert!(rows[1].ok);
        assert_eq!(rows[2].text, "Read: outside the checkout");
        assert!(!rows[2].ok);
    }

    #[test]
    fn partial_usage_names_every_unavailable_field() {
        let mut run = summary();
        run.input_tokens = Some(12);
        let label = usage_label(&run);
        assert!(label.contains("12 input tokens"));
        assert!(label.contains("Output tokens unavailable"));
        assert!(label.contains("Cached input tokens unavailable"));
        assert!(label.contains("Cost unavailable"));
    }

    #[test]
    fn truncated_final_result_uses_loaded_text_and_marks_missing_pages() {
        let mut run = summary();
        run.response = "bounded prefix".into();
        run.response_truncated = true;
        let events = [event(
            1,
            RunEvent::Text {
                text: "loaded stream".into(),
            },
        )];

        let (partial, notice) = response_text(&run, &events, true);
        assert_eq!(partial, "loaded stream");
        assert!(notice.is_some());

        let (complete, notice) = response_text(&run, &events, false);
        assert_eq!(complete, "loaded stream");
        assert!(notice.is_none());
    }
}
