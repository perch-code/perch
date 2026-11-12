//! One run's request, activity and result, in a sheet over the window.

use std::rc::Rc;
use std::sync::Arc;

use floem::event::EventPropagation;
use floem::peniko::kurbo::Point;
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{Position, TextOverflow};
use floem::text::{OverflowWrap, WordBreakStrength};
use floem::views::scroll::ScrollChanged;
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};
use perch_github::comments::{RunEvent, RunEventRecord};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::icon_action;

use super::model::{AgentRunPurpose, AgentRunStatus, AgentRunSummary};
use super::{status_color, text_action};

/// How wide the sheet is, at most.
const SHEET_WIDTH: f64 = 720.0;

/// A run's raw output, in a sheet over the window: what a reviewer said
/// as it read the change, opened from its line in the Review popover, or
/// what a question's run did when it has no thread to show it in. The
/// popover is too narrow for a transcript, and it is read once in a while,
/// so it comes up here over whatever opened it, which is as it was when
/// the sheet goes: on Escape, its close button, or a press beside it.
pub fn sheet(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let shown = Memo::new(move |_| review.agent_output.get());
    dyn_container(
        move || shown.get(),
        move |run_id| {
            let Some(run) =
                run_id.and_then(|id| floem::reactive::Effect::untrack(|| app.agent_run(&id)))
            else {
                return Empty::new().into_any();
            };
            Container::new(panel(app, review, backend.clone(), run))
                .on_event_stop(el::PointerDown, move |_, _| close(review))
                .style(|s| {
                    s.size_full()
                        .items_start()
                        .justify_center()
                        .padding(48.0)
                        .background(theme::scrim())
                })
                .into_any()
        },
    )
    .debug_name("agent-output")
    .style(move |s| {
        s.position(Position::Absolute)
            .inset(0.0)
            .z_index(100)
            .apply_if(shown.with(Option::is_none), |s| s.hide())
    })
}

/// Put the sheet away.
pub fn close(review: ReviewState) {
    if review.agent_output.with_untracked(Option::is_some) {
        review.agent_output.set(None);
    }
}

/// The sheet itself: who ran and what it was asked, a close button, and
/// what it did under them, scrolling inside the window's height.
fn panel(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    run: AgentRunSummary,
) -> impl IntoView {
    let name = crate::review::threads::agent_name(&run.backend, &run.model, &run.effort);
    let about = format!("{} \u{b7} {}", run.purpose.label(), run.subject());
    let close_button = icon_action(
        SidebarIcon::Close,
        "agent-output close",
        || true,
        move || close(review),
    );
    // The keyboard goes to the close button once it is laid out, so
    // Escape reaches the window from here whatever had it before.
    let close_id = close_button.view_id();
    let asked = RwSignal::new(false);
    let close_button = close_button.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if !asked.get_untracked() {
            asked.set(true);
            close_id.request_focus();
        }
    });
    let cap = (app.window_height.get_untracked() - 220.0).max(240.0);
    Stack::vertical((
        Stack::horizontal((
            Stack::vertical((
                Label::derived(move || name.clone()).style(|s| {
                    s.font_size(theme::TEXT_BODY)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg())
                        .selectable(false)
                }),
                Label::derived(move || about.clone()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_subtle())
                        .text_ellipsis()
                        .min_width(0.0)
                        .margin_top(2.0)
                        .selectable(false)
                }),
            ))
            .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0_f32)),
            close_button,
        ))
        .style(|s| s.items_start().width_full().gap(8.0).padding(14.0)),
        inline(app, backend, run, cap),
    ))
    .on_event_stop(el::PointerDown, |_, _| {})
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Escape && key.modifiers.is_empty() {
            close(review);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .debug_name("agent-output-panel")
    .style(|s| {
        s.flex_col()
            .width(SHEET_WIDTH)
            .max_width_full()
            .min_height(0.0)
            .apply(crate::ui::menu::panel_surface(floem::style::Style::new()))
    })
}

/// What a run did: its request, its activity, its answer and what it
/// cost, no taller than `cap` and scrolling inside that, following the
/// end while the run goes.
pub(super) fn inline(
    state: AppState,
    backend: Rc<Backend>,
    initial: AgentRunSummary,
    cap: f64,
) -> impl IntoView {
    let id = Arc::new(initial.id.clone());
    detail_body(state, backend, id, Arc::new(initial), cap).debug_name("agent-run-inline")
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

/// The sections, in a scroll that follows the end while the run goes, as
/// tall as what they hold up to `cap`.
fn detail_body(
    state: AppState,
    backend: Rc<Backend>,
    id: Arc<String>,
    initial: Arc<AgentRunSummary>,
    cap: f64,
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
        .style(move |s| {
            // The grey runs to the sheet's foot, so it takes the sheet's
            // round corners there, inside its ring.
            let radius = crate::ui::menu::RADIUS - 1.0;
            s.width_full()
                .max_height(cap)
                .background(theme::canvas_subtle())
                .border_bottom_left_radius(radius)
                .border_bottom_right_radius(radius)
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
    Stack::new((scroller, jump)).style(|s| s.flex_col().width_full().position(Position::Relative))
}

fn heading(text: &'static str) -> impl IntoView {
    Label::derived(move || text.to_string()).style(|s| {
        s.font_size(theme::TEXT_SECTION)
            .font_weight(theme::weight_emphasis())
            .color(theme::fg_subtle())
            .selectable(false)
    })
}

fn request_section(request: String) -> impl IntoView {
    Stack::vertical((
        heading("REQUEST"),
        Container::new(Label::derived(move || request.clone()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .color(theme::fg())
                .line_height(theme::PROSE_LINE_HEIGHT)
                .text_wrap()
                .width_full()
        }))
        .style(move |s| {
            s.width_full()
                .padding_left(12.0)
                .padding_vert(3.0)
                .border_left(2.0)
                .border_color(theme::accent())
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
                let page = state.agent_run_events_page_state(&page_state_id);
                let before = current_events(state, &page_state_id)
                    .first()
                    .map(|event| event.sequence);
                (page.has_more, page.loading, before)
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
                                theme::fg_subtle()
                            } else {
                                theme::closed()
                            })
                            .margin_top(6.0)
                            .flex_shrink(0.0_f32)
                    }),
                    Label::derived(move || row.text.clone()).style(move |s| {
                        s.font_size(theme::TEXT_BADGE)
                            .font_family(theme::CODE_FONT_STACK.to_string())
                            .color(if row.ok {
                                theme::fg_muted()
                            } else {
                                theme::closed()
                            })
                            .line_height(theme::PROSE_LINE_HEIGHT)
                            // A hash or a path in a command breaks at the
                            // column's edge rather than running past it.
                            .text_overflow(TextOverflow::Wrap {
                                overflow_wrap: OverflowWrap::Anywhere,
                                word_break: WordBreakStrength::Normal,
                            })
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
                let loading = state.agent_run_events_page_state(&empty_id).loading;
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
                    .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_subtle()))
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
                    // The code font has no `↳`: it drew a blank.
                    if parent_id.is_some() {
                        format!("└ {summary}")
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
                RunEvent::Steer { text } => (
                    format!("You: {}", text.lines().next().unwrap_or_default()),
                    true,
                ),
                RunEvent::Completed { .. } => ("Completed".into(), true),
                RunEvent::Terminated { message, .. } => (message.clone(), false),
                RunEvent::LegacyStep { summary, ok } => (summary.clone(), *ok),
                RunEvent::Text { .. } | RunEvent::SteerLanded { .. } | RunEvent::Usage { .. } => {
                    return None
                }
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
                state.agent_run_events_page_state(&page_state_id).has_more,
            )
        },
        |(run, events, has_more)| result_content(run, events, has_more).into_any(),
    )
}

/// The stored response as it is read. A review keeps the findings it filed
/// and their verification after its prose, as data for extraction.
fn answer(run: &AgentRunSummary) -> &str {
    if run.purpose == AgentRunPurpose::Review {
        crate::backend::reviews::reviewer_prose(&run.response)
    } else {
        &run.response
    }
}

fn response_text(
    run: &AgentRunSummary,
    events: &[RunEventRecord],
    has_more: bool,
) -> (String, Option<&'static str>) {
    let response = answer(run);
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
                response.to_string(),
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
    if response.trim().is_empty() {
        (streamed, None)
    } else {
        (response.to_string(), None)
    }
}

fn result_content(
    run: AgentRunSummary,
    events: Arc<Vec<RunEventRecord>>,
    has_more: bool,
) -> impl IntoView {
    let (result, incomplete) = response_text(&run, &events, has_more);
    let repository = run.repository.clone();
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
                    .color(outcome_color())
                    .line_height(theme::PROSE_LINE_HEIGHT)
                    .text_wrap()
                    .width_full()
            }))
            .style(move |s| {
                s.width_full()
                    .padding(11.0)
                    .background(outcome_color().with_alpha(0.07))
                    .border(1.0)
                    .border_color(outcome_color().with_alpha(0.30))
                    .border_radius(theme::RADIUS)
            }),
            crate::review::markdown::render_in(&result, &repository),
        ))
        .style(|s| s.flex_col().width_full().gap(12.0))
        .into_any(),
        (Some(error), true) => {
            Container::new(Label::derived(move || error.clone()).style(move |s| {
                s.font_size(theme::TEXT_BODY)
                    .color(outcome_color())
                    .line_height(theme::PROSE_LINE_HEIGHT)
                    .text_wrap()
                    .width_full()
            }))
            .style(move |s| {
                s.width_full()
                    .padding(11.0)
                    .background(outcome_color().with_alpha(0.07))
                    .border(1.0)
                    .border_color(outcome_color().with_alpha(0.30))
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
        .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_subtle()))
        .into_any(),
        (None, false) => crate::review::markdown::render_in(&result, &repository).into_any(),
    };
    let content = if let Some(message) = incomplete {
        Stack::vertical((
            Container::new(Label::derived(move || message.to_string()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_subtle())
                    .line_height(theme::PROSE_LINE_HEIGHT)
                    .text_wrap()
                    .width_full()
            }))
            .style(|s| {
                s.width_full()
                    .padding(9.0)
                    .background(theme::badge_bg())
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
                .color(theme::fg_subtle())
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
            .border_color(theme::border_muted())
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
    // Claude Code bills work it did somewhere the parent's counters do not
    // see, such as a subagent, as a cost against zero tokens. Zero tokens
    // with a real cost is that, not a free run: show the cost alone.
    let counted = [run.input_tokens, run.output_tokens, run.cached_input_tokens]
        .iter()
        .flatten()
        .any(|tokens| *tokens > 0);
    if let (false, Some(cost)) = (counted, run.cost_usd) {
        if cost > 0.0 {
            return format!("${cost:.2} · token counts unavailable");
        }
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
        assert_eq!(rows[0].text, "└ Inspect parser");
        assert_eq!(rows[1].text, "Parser inspected");
        assert!(rows[1].ok);
        assert_eq!(rows[2].text, "Read: outside the checkout");
        assert!(!rows[2].ok);
    }

    #[test]
    fn a_cost_against_zero_tokens_shows_the_cost_alone() {
        let mut run = summary();
        run.input_tokens = Some(0);
        run.output_tokens = Some(0);
        run.cached_input_tokens = Some(0);
        run.cost_usd = Some(0.791728);
        assert_eq!(usage_label(&run), "$0.79 · token counts unavailable");

        // A zero cost against zero tokens is a run that did nothing, and
        // says so with its numbers.
        run.cost_usd = Some(0.0);
        assert!(usage_label(&run).starts_with("0 input tokens"));
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

    /// A finished run's detail at `width`, with `events` and `response`.
    fn detail_at(
        width: f64,
        events: Vec<RunEventRecord>,
        response: &str,
    ) -> floem::headless::HeadlessHarness {
        let root = floem::headless::TestRoot::new();
        let app = AppState::new();
        let record = perch_github::RunRecord {
            id: "run".into(),
            backend: "claude-code".into(),
            model: "claude-opus-5".into(),
            effort: "high".into(),
            status: perch_github::RunStatus::Completed,
            final_result: Some(response.to_string()),
            queued_at: jiff::Timestamp::UNIX_EPOCH,
            ..perch_github::RunRecord::default()
        };
        app.merge_agent_runs([perch_github::RunSummary::from(&record)]);
        app.set_agent_run_detail(&record, events, false);
        let run = app.agent_run("run").unwrap();
        let view = inline(app, Rc::new(Backend::detached()), run, 100_000.0);
        let mut harness = floem::headless::HeadlessHarness::new_with_size(root, view, width, 800.0);
        harness.rebuild();
        harness.paint();
        harness
    }

    /// The text view whose text holds `needle`.
    fn text_holding(id: floem::ViewId, needle: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(needle) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| text_holding(child, needle))
    }

    /// How tall `text` is in the code font, wrapped at `width`.
    fn code_height(text: &str, size: f32, leading: f32, wrap: OverflowWrap, width: f64) -> f64 {
        use floem::text::{
            Attrs, AttrsList, FamilyOwned, LineHeightValue, TextLayout, TextWrapMode,
        };
        let family: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect();
        let attrs = Attrs::new()
            .font_size(size)
            .family(&family)
            .line_height(LineHeightValue::Normal(leading));
        let mut layout = TextLayout::new();
        layout.set_text_wrap_mode(TextWrapMode::Wrap);
        layout.set_overflow_wrap(wrap);
        layout.set_text(text, AttrsList::new(attrs), None);
        layout.set_size(width as f32, f32::MAX);
        layout.size().height
    }

    /// A command with a hash and a path in it is wider than the drawer at
    /// its narrowest. It breaks at the column's edge and its row is as tall
    /// as its lines. Its text used to stay as wide as the path, running
    /// past the edge, and the row could take the height of the command
    /// broken after every word: a dozen empty lines under three.
    #[test]
    fn a_long_command_breaks_at_the_column_and_its_row_fits_its_lines() {
        let command = "Ran git show 629af7af4a76c2ba32eb0c68a7262f9799c6deeb:slatedb/src/format/sst.rs | sed -n '520,600p'";
        let call = RunEvent::ToolCall {
            id: "call".into(),
            name: "Bash".into(),
            summary: command.into(),
            parent_id: Some("task".into()),
        };
        let harness = detail_at(theme::DRAWER_MIN_WIDTH, vec![event(1, call)], "Done.");
        let row = text_holding(harness.root_id(), "629af7af").expect("the row is drawn");
        let content = harness.get_content_rect(row);
        let taffy = row.taffy();
        let text = taffy
            .borrow()
            .layout(taffy.borrow().children(row.taffy_node()).unwrap()[0])
            .unwrap()
            .size;
        assert!(
            f64::from(text.width) <= content.width() + 0.5,
            "the text stays in its column: {text:?} in {content:?}"
        );
        let want = code_height(
            &format!("└ {command}"),
            theme::TEXT_BADGE,
            theme::PROSE_LINE_HEIGHT,
            OverflowWrap::Anywhere,
            content.width(),
        );
        assert!(
            (content.height() - want).abs() < 0.5,
            "the row is as tall as its lines ({want}), not {}",
            content.height()
        );
    }

    /// A fenced block in the answer with a word wider than its column is as
    /// tall as its lines. It was once as tall as the block broken after
    /// every word, 42,000px for a review's findings, most of it an empty
    /// box under the code.
    #[test]
    fn a_code_block_with_a_long_word_is_as_tall_as_its_lines() {
        let word = "test_alignment_larger_than_block_size_gives_each_block_a_unit";
        let code = format!(
            "{{\n  \"failure_scenario\": \"{word} records it as correct, and it wastes the difference in every block\"\n}}"
        );
        let response = format!("Here are the findings.\n\n```json\n{code}\n```\n\nDone.");
        let harness = detail_at(theme::DRAWER_MIN_WIDTH, Vec::new(), &response);
        let block = text_holding(harness.root_id(), word).expect("the block is drawn");
        let content = harness.get_content_rect(block);
        let want = code_height(
            &code,
            theme::PROSE_CODE_FONT_SIZE,
            theme::PROSE_CODE_LINE_HEIGHT,
            OverflowWrap::Normal,
            content.width(),
        );
        assert!(
            (content.height() - want).abs() < 0.5,
            "the block is as tall as its lines ({want}), not {}",
            content.height()
        );
    }

    #[test]
    fn a_review_reads_as_its_prose_without_the_findings_it_filed() {
        let report = serde_json::json!({ "findings": [{ "file": "src/lib.rs", "line": 4 }] });
        let mut run = summary();
        run.purpose = AgentRunPurpose::Review;
        run.response = perch_agent::preserve_native_response("Reported 1 finding.", Some(&report));
        assert_eq!(response_text(&run, &[], false).0, "Reported 1 finding.");

        // Only a review files one: an answer that quotes the marker keeps it.
        run.purpose = AgentRunPurpose::Question;
        assert!(response_text(&run, &[], false).0.contains("\"findings\""));
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
