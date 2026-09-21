//! Active and History lists in the Agents inspector.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::menu::Menu;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, sidebar_glyph};

use super::model::{
    active_groups_with_reviews, elapsed_label, group_review_runs, history_runs_with_reviews,
    AgentRetryTarget, AgentReviewRunGroup, AgentRunEntry, AgentRunGroup, AgentRunStatus,
    AgentRunSummary, AgentsTab, BackendFilter, OutcomeFilter, WorktreeFilter,
};
use super::{metadata, navigate_to_source, request_agent_runs, status_color, text_action};

pub fn view(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let filters_backend = backend.clone();
    let body_backend = backend.clone();
    Stack::vertical((
        tabs(state, backend),
        filters(state, filters_backend),
        dyn_container(
            move || state.agents_tab.get(),
            move |tab| match tab {
                AgentsTab::Active => active(state, review, body_backend.clone()).into_any(),
                AgentsTab::History => history(state, review, body_backend.clone()).into_any(),
            },
        )
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
    ))
    .style(|s| s.size_full().flex_col().background(theme::CANVAS_SUBTLE))
    .debug_name("agents-list")
}

fn tabs(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    Stack::horizontal(AgentsTab::ALL.map(|tab| {
        let tab_backend = backend.clone();
        let action = Rc::new(move || {
            state.agents_tab.set(tab);
            state.selected_agent_run.set(None);
            if tab == AgentsTab::History {
                request_agent_runs(state, &tab_backend, 50);
            }
        });
        let click = action.clone();
        Stack::horizontal((
            Label::derived(move || tab.label().to_string()).style(move |s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(if state.agents_tab.get() == tab {
                        theme::weight_emphasis()
                    } else {
                        theme::weight_body()
                    })
                    .color(if state.agents_tab.get() == tab {
                        theme::FG
                    } else {
                        theme::FG_MUTED
                    })
                    .selectable(false)
                    .pointer_events_none()
            }),
            dyn_container(
                move || match tab {
                    AgentsTab::Active => state
                        .agent_runs
                        .with(|runs| runs.iter().filter(|run| run.status.is_active()).count()),
                    AgentsTab::History => {
                        let groups = state.review_groups.with(|all| {
                            all.values()
                                .flat_map(|groups| groups.iter().cloned())
                                .collect::<Vec<_>>()
                        });
                        state.agent_runs.with(|runs| {
                            history_runs_with_reviews(
                                runs,
                                None,
                                WorktreeFilter::All,
                                BackendFilter::All,
                                OutcomeFilter::All,
                                &groups,
                            )
                            .len()
                        })
                    }
                },
                move |count| {
                    if count == 0 {
                        Empty::new().into_any()
                    } else {
                        Label::derived(move || count.to_string())
                            .style(|s| {
                                s.font_size(theme::TEXT_SECTION)
                                    .font_weight(theme::weight_emphasis())
                                    .color(theme::FG_MUTED)
                                    .height(18.0)
                                    .padding_horiz(6.0)
                                    .border_radius(9.0)
                                    .background(theme::BADGE_BG)
                                    .items_center()
                                    .justify_center()
                                    .margin_left(6.0)
                                    .selectable(false)
                                    .pointer_events_none()
                            })
                            .into_any()
                    }
                },
            )
            .style(|s| s.pointer_events_none()),
        ))
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
                .height(theme::TAB_HEIGHT)
                .items_center()
                .padding_horiz(10.0)
                .border_bottom(2.0)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING));
            if state.agents_tab.get() == tab {
                base.border_color(theme::ACCENT)
            } else {
                base.border_color(floem::peniko::Color::TRANSPARENT)
                    .hover(|s| s.border_color(theme::BORDER))
            }
        })
        .debug_name(format!("agents-tab:{}", tab.label()))
    }))
    .style(|s| {
        s.height(theme::TAB_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_end()
            .padding_left(10.0)
            .gap(2.0)
            .background(theme::CANVAS_SUBTLE)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

fn filters(state: AppState, _backend: Rc<Backend>) -> impl IntoView {
    Stack::horizontal((
        worktree_filter(state),
        backend_filter(state),
        dyn_container(
            move || state.agents_tab.get(),
            move |tab| {
                if tab == AgentsTab::History {
                    outcome_filter(state).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        ),
    ))
    .style(|s| {
        s.width_full()
            .height(44.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .gap(6.0)
            .padding_horiz(12.0)
            .background(theme::CANVAS)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED)
    })
}

fn worktree_filter(state: AppState) -> impl IntoView {
    filter_button(
        move || state.agents_worktree_filter.get().label().to_string(),
        move || {
            let selected = state.agents_worktree_filter.get_untracked();
            let current = state.open_item.with_untracked(Option::is_some);
            let mut menu = Menu::new();
            for value in WorktreeFilter::ALL {
                menu = menu.item(value.label(), move |item| {
                    item.checked(value == selected)
                        .enabled(value == WorktreeFilter::All || current)
                        .action(move || state.agents_worktree_filter.set(value))
                });
            }
            menu
        },
    )
}

fn backend_filter(state: AppState) -> impl IntoView {
    filter_button(
        move || state.agents_backend_filter.get().label().to_string(),
        move || {
            let selected = state.agents_backend_filter.get_untracked();
            let mut menu = Menu::new();
            for value in BackendFilter::ALL {
                menu = menu.item(value.label(), move |item| {
                    item.checked(value == selected)
                        .action(move || state.agents_backend_filter.set(value))
                });
            }
            menu
        },
    )
}

fn outcome_filter(state: AppState) -> impl IntoView {
    filter_button(
        move || state.agents_outcome_filter.get().label().to_string(),
        move || {
            let selected = state.agents_outcome_filter.get_untracked();
            let mut menu = Menu::new();
            for value in OutcomeFilter::ALL {
                menu = menu.item(value.label(), move |item| {
                    item.checked(value == selected)
                        .action(move || state.agents_outcome_filter.set(value))
                });
            }
            menu
        },
    )
}

fn filter_button(
    label: impl Fn() -> String + 'static,
    menu: impl Fn() -> Menu + 'static,
) -> impl IntoView {
    Stack::horizontal((
        Label::derived(label).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::FG_MUTED)
                .selectable(false)
                .text_ellipsis()
                .min_width(0.0)
                .pointer_events_none()
                .margin_right(5.0)
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE)
            .style(|s| s.size(9.0, 9.0).pointer_events_none()),
    ))
    .popout_menu(menu)
    .style(|s| {
        clickable(s)
            .height(27.0)
            .max_width(150.0)
            .min_width(0.0)
            .items_center()
            .padding_horiz(8.0)
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
}

fn active(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || {
            (
                state.agent_runs.get(),
                state.open_item.get(),
                state.agents_worktree_filter.get(),
                state.agents_backend_filter.get(),
                state.agent_history_loading.get(),
                state.agent_history_load_failed.get(),
                state.review_groups.get(),
            )
        },
        move |(runs, current, worktrees, backends, loading, failed, review_groups)| {
            let summaries = review_groups
                .values()
                .flat_map(|groups| groups.iter().cloned())
                .collect::<Vec<_>>();
            let groups = active_groups_with_reviews(
                &runs,
                current.as_deref(),
                worktrees,
                backends,
                &summaries,
            );
            if groups.is_empty() {
                if loading {
                    return empty_state("Loading agents…", "Reading active and queued work.")
                        .into_any();
                }
                if failed {
                    return load_error(state, backend.clone(), 50).into_any();
                }
                return empty_state(
                    "No agents running",
                    "Questions and reviews appear here while they work.",
                )
                .into_any();
            }
            let group_backend = backend.clone();
            dyn_stack(
                move || groups.clone(),
                |group| {
                    (
                        group.item_id.clone(),
                        group
                            .runs
                            .iter()
                            .map(|run| run.id.clone())
                            .collect::<Vec<_>>(),
                    )
                },
                move |group| {
                    let summaries = group
                        .item_id
                        .as_deref()
                        .and_then(|item_id| review_groups.get(item_id))
                        .map(|summaries| summaries.as_ref().clone())
                        .unwrap_or_default();
                    group_view(state, review, group_backend.clone(), group, summaries)
                },
            )
            .style(|s| s.flex_col().width_full().padding_bottom(12.0))
            .scroll()
            .style(|s| s.size_full().min_height(0.0))
            .into_any()
        },
    )
    .style(|s| s.size_full().min_height(0.0))
}

fn group_view(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    group: AgentRunGroup,
    review_groups: Vec<crate::review::model::ReviewGroupSummary>,
) -> impl IntoView {
    let count = group.runs.len();
    let label = group.label.clone();
    let current = group.current;
    let entries = group_review_runs(&group.runs, &review_groups);
    Stack::vertical((
        Stack::horizontal((
            dyn_container(
                move || current,
                move |current| {
                    if current {
                        Label::derived(|| "Current".to_string())
                            .style(|s| {
                                s.font_size(theme::TEXT_SECTION)
                                    .font_weight(theme::weight_emphasis())
                                    .color(theme::ACCENT)
                                    .margin_right(7.0)
                                    .selectable(false)
                            })
                            .into_any()
                    } else {
                        Empty::new().into_any()
                    }
                },
            ),
            Label::derived(move || label.clone()).style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG_SUBTLE)
                    .text_ellipsis()
                    .min_width(0.0)
                    .selectable(false)
            }),
            Label::derived(move || count.to_string()).style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .color(theme::FG_SUBTLE)
                    .margin_left(6.0)
                    .selectable(false)
            }),
        ))
        .style(|s| {
            s.items_center()
                .width_full()
                .padding_left(14.0)
                .padding_right(12.0)
                .padding_top(14.0)
                .padding_bottom(5.0)
        }),
        dyn_stack(
            move || entries.clone(),
            |entry| entry.key(),
            move |entry| run_entry(state, review, backend.clone(), entry),
        )
        .style(|s| s.flex_col().width_full()),
    ))
    .style(|s| s.flex_col().width_full())
}

fn history(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || {
            (
                state.agent_runs.get(),
                state.open_item.get(),
                state.agents_worktree_filter.get(),
                state.agents_backend_filter.get(),
                state.agents_outcome_filter.get(),
                state.agent_history_loading.get(),
                state.agent_history_has_more.get(),
                state.agent_history_load_failed.get(),
                state.review_groups.get(),
            )
        },
        move |(
            runs,
            current,
            worktrees,
            backends,
            outcomes,
            loading,
            has_more,
            failed,
            review_groups,
        )| {
            let summaries = review_groups
                .values()
                .flat_map(|groups| groups.iter().cloned())
                .collect::<Vec<_>>();
            let rows = history_runs_with_reviews(
                &runs,
                current.as_deref(),
                worktrees,
                backends,
                outcomes,
                &summaries,
            );
            let has_rows = !rows.is_empty();
            let loaded = runs.iter().filter(|run| !run.status.is_active()).count();
            let entries = group_review_runs(&rows, &summaries);
            let more_backend = backend.clone();
            let row_backend = backend.clone();
            let body = if rows.is_empty() {
                if failed {
                    load_error(state, backend.clone(), loaded.max(50)).into_any()
                } else {
                    let (title, message) = if loading {
                        ("Loading history…", "Reading saved agent runs.")
                    } else if has_more {
                        (
                            "No matches yet",
                            "Older saved runs may match these filters.",
                        )
                    } else {
                        (
                            "No saved runs",
                            "Completed and stopped work will remain available here.",
                        )
                    };
                    empty_state(title, message).into_any()
                }
            } else {
                dyn_stack(
                    move || entries.clone(),
                    |entry| entry.key(),
                    move |entry| run_entry(state, review, row_backend.clone(), entry),
                )
                .style(|s| s.flex_col().width_full())
                .into_any()
            };
            Stack::vertical((
                body,
                dyn_container(
                    move || (has_more, loading, failed, has_rows),
                    move |(has_more, loading, failed, has_rows)| {
                        if (!has_more && !failed) || (failed && !has_rows) {
                            return Empty::new().into_any();
                        }
                        let more_backend = more_backend.clone();
                        text_action(
                            move || {
                                if loading {
                                    "Loading…".to_string()
                                } else if failed {
                                    "Retry loading runs".to_string()
                                } else {
                                    "Load older runs".to_string()
                                }
                            },
                            false,
                            move || {
                                if !loading {
                                    request_agent_runs(state, &more_backend, loaded + 50);
                                }
                            },
                        )
                        .style(|s| s.margin(12.0))
                        .into_any()
                    },
                ),
            ))
            .style(|s| s.flex_col().width_full().items_center())
            .scroll()
            .style(|s| s.size_full().min_height(0.0))
            .into_any()
        },
    )
    .style(|s| s.size_full().min_height(0.0))
}

fn run_entry(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    entry: AgentRunEntry,
) -> floem::AnyView {
    match entry {
        AgentRunEntry::Run(run) => run_row(state, review, backend, run).into_any(),
        AgentRunEntry::ReviewGroup(group) => {
            review_group_row(state, review, backend, group).into_any()
        }
    }
}

fn review_group_row(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    group: AgentReviewRunGroup,
) -> impl IntoView {
    let group_key = group.key.clone();
    let icon_key = group_key.clone();
    let toggle_key = group_key.clone();
    let keyboard_key = group_key.clone();
    let debug_key = group_key.clone();
    let child_runs = group.runs.clone();
    let child_backend = backend.clone();
    let count = child_runs.len();
    let (title, status) = match group.summary.as_ref() {
        Some(summary) => {
            let title = format!(
                "Review #{} · {} · against {}",
                summary.group.number,
                short_sha(&summary.snapshot.head_sha),
                summary.snapshot.requested_base_ref
            );
            let status = if summary.group.status.is_active() {
                let complete = summary
                    .attempts
                    .iter()
                    .filter(|attempt| attempt.phase.is_terminal())
                    .count();
                format!("{complete} of {} complete", summary.attempts.len())
            } else {
                title_case_status(summary.group.status.name())
            };
            (title, status)
        }
        None => (
            "Worktree review".to_string(),
            format!("{count} reviewer{}", if count == 1 { "" } else { "s" }),
        ),
    };
    let representative = child_runs.first().cloned();
    let view = match representative {
        Some(run)
            if run.source_available
                && run
                    .item_id
                    .as_deref()
                    .is_some_and(|item_id| state.item(item_id).is_some()) =>
        {
            let view_backend = backend.clone();
            text_action(
                || "View".to_string(),
                true,
                move || navigate_to_source(state, review, &view_backend, &run),
            )
            .into_any()
        }
        _ => Empty::new().into_any(),
    };
    let toggle = move |key: &str| {
        state.collapsed_agent_review_groups.update(|collapsed| {
            if !collapsed.insert(key.to_string()) {
                collapsed.remove(key);
            }
        });
    };

    let header = Stack::horizontal((
        dyn_container(
            move || {
                state
                    .collapsed_agent_review_groups
                    .with(|collapsed| collapsed.contains(&icon_key))
            },
            move |collapsed| {
                sidebar_glyph(
                    if collapsed {
                        SidebarIcon::ChevronRight
                    } else {
                        SidebarIcon::ChevronDown
                    },
                    theme::FG_SUBTLE,
                )
                .style(|s| s.size(10.0, 10.0).pointer_events_none())
            },
        ),
        Stack::vertical((
            Label::derived(move || title.clone()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG)
                    .text_ellipsis()
                    .min_width(0.0)
                    .width_full()
                    .selectable(false)
                    .pointer_events_none()
            }),
            Label::derived(move || status.clone()).style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG_MUTED)
                    .selectable(false)
                    .pointer_events_none()
            }),
        ))
        .style(|s| s.flex_col().gap(2.0).min_width(0.0).flex_grow(1.0_f32)),
        view,
    ))
    .on_event_stop(el::Click, move |_, _| toggle(&toggle_key))
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            toggle(&keyboard_key);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| {
        clickable(s)
            .width_full()
            .min_height(48.0)
            .items_center()
            .gap(9.0)
            .padding_left(14.0)
            .padding_right(10.0)
            .padding_vert(8.0)
            .background(theme::CANVAS_SUBTLE)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
    .debug_name(format!("agent-review-group:{debug_key}"));

    let children = dyn_container(
        move || {
            state
                .collapsed_agent_review_groups
                .with(|collapsed| collapsed.contains(&group_key))
        },
        move |collapsed| {
            if collapsed {
                Empty::new().into_any()
            } else {
                let child_backend = child_backend.clone();
                let visible_runs = child_runs.clone();
                dyn_stack(
                    move || visible_runs.clone(),
                    |run| (run.id.clone(), run.status, run.latest_activity.clone()),
                    move |run| run_row(state, review, child_backend.clone(), run),
                )
                .style(|s| {
                    s.flex_col()
                        .width_full()
                        .margin_left(12.0)
                        .border_left(1.0)
                        .border_color(theme::BORDER)
                })
                .into_any()
            }
        },
    );

    Stack::vertical((header, children)).style(|s| s.flex_col().width_full())
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn title_case_status(status: &str) -> String {
    let mut chars = status.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn run_row(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    run: AgentRunSummary,
) -> impl IntoView {
    let id = run.id.clone();
    let debug_id = id.clone();
    let focus_id = id.clone();
    let select_id = id.clone();
    let detail_backend = backend.clone();
    let select: Rc<dyn Fn()> = Rc::new(move || {
        state.selected_agent_run.set(Some(select_id.clone()));
        state.set_agent_run_events_loading(&select_id);
        detail_backend.load_agent_run_detail(&select_id);
    });
    let click = select.clone();
    let color = status_color(run.status);
    let meta = metadata(&run);
    let subject = run.subject();
    let activity = run
        .latest_activity
        .clone()
        .unwrap_or_else(|| match run.status {
            AgentRunStatus::Queued => "Waiting for an execution slot".to_string(),
            AgentRunStatus::Starting => "Starting the agent".to_string(),
            AgentRunStatus::Running => "Working…".to_string(),
            AgentRunStatus::Stopping => "Stopping the process tree…".to_string(),
            AgentRunStatus::Completed => "Completed".to_string(),
            AgentRunStatus::Failed => run.terminal_display().unwrap_or_else(|| "Failed".into()),
            AgentRunStatus::Stopped => run
                .terminal_display()
                .unwrap_or_else(|| "Stopped".to_string()),
            AgentRunStatus::Interrupted => run
                .terminal_display()
                .unwrap_or_else(|| "Interrupted".to_string()),
        });
    let elapsed_run = run.clone();
    let queue = queue_position(state, &run.id);
    let status = run.status;
    let action_run = run.clone();
    let footer = {
        let group = action_run.group_label();
        if group.is_empty() {
            action_run.purpose.label().to_string()
        } else {
            format!("{} · {group}", action_run.purpose.label())
        }
    };

    let row = Stack::vertical((
        Stack::horizontal((
            Empty::new().style(move |s| {
                s.size(7.0, 7.0)
                    .border_radius(4.0)
                    .background(color)
                    .flex_shrink(0.0_f32)
            }),
            Label::derived(move || meta.clone()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG_MUTED)
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
                    .selectable(false)
                    .pointer_events_none()
            }),
            Label::derived(move || {
                if status == AgentRunStatus::Queued {
                    let queued_for = elapsed_label(&elapsed_run, state.now.get());
                    let queued_for = queued_for.strip_prefix("Queued ").unwrap_or(&queued_for);
                    match queue {
                        Some(position) => format!("Queue #{position} · {queued_for}"),
                        None => format!("Queued · {queued_for}"),
                    }
                } else {
                    format!(
                        "{} · {}",
                        status.label(),
                        elapsed_label(&elapsed_run, state.now.get())
                    )
                }
            })
            .style(move |s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(theme::weight_emphasis())
                    .color(color)
                    .flex_shrink(0.0_f32)
                    .selectable(false)
                    .pointer_events_none()
            }),
        ))
        .style(|s| s.items_center().width_full().gap(7.0).pointer_events_none()),
        Label::derived(move || subject.clone()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG)
                .text_ellipsis()
                .width_full()
                .selectable(false)
                .pointer_events_none()
        }),
        Label::derived(move || activity.clone()).style(move |s| {
            s.font_size(theme::TEXT_BADGE)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(if matches!(status, AgentRunStatus::Failed) {
                    theme::CLOSED
                } else {
                    theme::FG_SUBTLE
                })
                .text_ellipsis()
                .width_full()
                .selectable(false)
                .pointer_events_none()
        }),
        Stack::horizontal((
            Label::derived(move || footer.clone()).style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .color(theme::FG_SUBTLE)
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
                    .selectable(false)
                    .pointer_events_none()
            }),
            row_actions(state, review, backend, run),
        ))
        .style(|s| s.items_center().width_full().height(27.0)),
    ))
    .on_event_stop(el::Click, move |_, _| click())
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            select();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(move |s| {
        let selected = state
            .selected_agent_run
            .with(|selected| selected.as_deref() == Some(id.as_str()));
        let base = clickable(s)
            .width_full()
            .min_height(theme::AGENT_ROW_MIN_HEIGHT)
            .flex_col()
            .gap(6.0)
            .padding_vert(10.0)
            .padding_left(14.0)
            .padding_right(10.0)
            .border_left(2.0)
            .border_bottom(1.0)
            .border_bottom_color(theme::BORDER_MUTED)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .hover(|s| s.background(theme::CANVAS_HOVER));
        if selected {
            base.background(theme::ACCENT_SUBTLE)
                .border_left_color(theme::ACCENT)
        } else {
            base.background(theme::CANVAS)
                .border_left_color(floem::peniko::Color::TRANSPARENT)
        }
    })
    .debug_name(format!("agent-run:{debug_id}"));
    let row_id = row.view_id();
    let focus_row = state
        .pending_agent_row_focus
        .with_untracked(|pending| pending.as_deref() == Some(focus_id.as_str()));
    if focus_row {
        state.pending_agent_row_focus.set(None);
    }
    let asked = std::rc::Rc::new(std::cell::Cell::new(false));
    row.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if focus_row && !asked.replace(true) {
            row_id.request_focus();
        }
    })
}

fn row_actions(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    run: AgentRunSummary,
) -> impl IntoView {
    let source_available = run.source_available
        && run
            .item_id
            .as_deref()
            .is_some_and(|item_id| state.item(item_id).is_some());
    let primary = if source_available {
        let view_run = run.clone();
        let view_backend = backend.clone();
        text_action(
            || "View".to_string(),
            true,
            move || navigate_to_source(state, review, &view_backend, &view_run),
        )
        .into_any()
    } else {
        Label::derived(|| "Source unavailable".to_string())
            .style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .color(theme::FG_SUBTLE)
                    .padding_horiz(7.0)
                    .selectable(false)
            })
            .into_any()
    };
    let secondary = match run.status {
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
        status if status.may_retry() => match run.retry_target() {
            AgentRetryTarget::ReviewAttempt(attempt_id) => text_action(
                || "Retry".to_string(),
                false,
                move || backend.retry_review_attempt(&attempt_id),
            )
            .into_any(),
            AgentRetryTarget::Run(id) => text_action(
                || "Retry".to_string(),
                false,
                move || backend.retry_run(&id),
            )
            .into_any(),
        },
        AgentRunStatus::Stopping => Label::derived(|| "Stopping…".to_string())
            .style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::FG_SUBTLE)
                    .selectable(false)
                    .padding_horiz(8.0)
            })
            .into_any(),
        _ => Empty::new().into_any(),
    };
    Stack::horizontal((primary, secondary))
        .style(|s| s.items_center().gap(2.0).flex_shrink(0.0_f32))
}

fn queue_position(state: AppState, id: &str) -> Option<usize> {
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

fn empty_state(title: &'static str, body: &'static str) -> impl IntoView {
    Container::new(
        Stack::vertical((
            Container::new(sidebar_glyph(SidebarIcon::Agents, theme::FG_SUBTLE)).style(|s| {
                s.size(44.0, 44.0)
                    .border_radius(22.0)
                    .background(theme::CANVAS)
                    .border(1.0)
                    .border_color(theme::BORDER_MUTED)
                    .items_center()
                    .justify_center()
                    .margin_bottom(13.0)
            }),
            Label::derived(move || title.to_string()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG)
                    .margin_bottom(5.0)
            }),
            Label::derived(move || body.to_string()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .text_align(floem::text::Alignment::Center)
                    .max_width(250.0)
                    .text_wrap()
            }),
        ))
        .style(|s| s.items_center()),
    )
    .style(|s| s.size_full().items_center().justify_center().padding(32.0))
}

fn load_error(state: AppState, backend: Rc<Backend>, history_limit: usize) -> impl IntoView {
    Container::new(
        Stack::vertical((
            Label::derived(|| "Could not load agents".to_string()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG)
            }),
            Label::derived(|| {
                "The saved run list did not answer. Existing work continues in the background."
                    .to_string()
            })
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .text_align(floem::text::Alignment::Center)
                    .text_wrap()
                    .max_width(270.0)
            }),
            text_action(
                || "Retry".to_string(),
                true,
                move || request_agent_runs(state, &backend, history_limit),
            ),
        ))
        .style(|s| s.flex_col().items_center().gap(10.0)),
    )
    .style(|s| s.size_full().items_center().justify_center().padding(32.0))
}
