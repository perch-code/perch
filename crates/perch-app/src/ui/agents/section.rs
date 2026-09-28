//! An item's runs, on its Conversation: the Agents section over REVIEW
//! HISTORY, whose header says what they are doing and folds the list
//! under it.
//!
//! The list is the item's runs, working first and then its history,
//! with a review's attempts under a row for the review. A run opens in
//! place to what it did: its request, its activity, its answer and what
//! it cost. The row is an index of the item's runs, not a second copy of
//! its thread, which has a way to it.

use std::collections::HashSet;
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::kurbo::Rect;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, dyn_stack, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::doc::CardId;
use crate::review::model::{ReviewGroupSummary, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, sidebar_glyph};

use super::model::{
    elapsed_label, group_review_runs, item_runs, AgentRetryTarget, AgentReviewRunGroup,
    AgentRunEntry, AgentRunStatus, AgentRunSummary, RunCounts,
};
use super::{detail, metadata, queue_position, status_color, text_action};

/// How tall an open run is before what it did scrolls inside it.
pub const OPEN_RUN_MAX_HEIGHT: f64 = 480.0;

/// The open item's id. Tracked.
fn item_id(review: ReviewState) -> Option<String> {
    review
        .item
        .with(|item| item.as_ref().map(|item| item.id.clone()))
}

/// The open item's review groups. Tracked.
fn review_groups(app: AppState, item_id: &str) -> Vec<ReviewGroupSummary> {
    app.review_groups
        .with(|all| all.get(item_id).map(|groups| groups.as_ref().clone()))
        .unwrap_or_default()
}

/// The open item's runs as the list has them. Tracked.
fn runs_of(app: AppState, review: ReviewState) -> Vec<AgentRunSummary> {
    let Some(item_id) = item_id(review) else {
        return Vec::new();
    };
    let groups = review_groups(app, &item_id);
    app.agent_runs
        .with(|runs| item_runs(runs, &item_id, &groups))
}

/// The runs waiting on the reader to answer a request. Tracked.
fn waiting(review: ReviewState) -> HashSet<String> {
    review
        .approvals
        .with(|pending| pending.iter().map(|p| p.run_id.clone()).collect())
}

/// Whether the open item has had a run. Tracked.
fn present(app: AppState, review: ReviewState) -> bool {
    let Some(item_id) = item_id(review) else {
        return false;
    };
    app.agent_runs.with(|runs| {
        runs.iter()
            .any(|run| run.item_id.as_deref() == Some(item_id.as_str()))
    })
}

/// The section's header: its name and what the runs are doing, the
/// busiest first, and a chevron. It folds and unfolds the list. The
/// item's runs are asked for as it opens, the first time.
fn section_header(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let item = Memo::new(move |_| item_id(review));
    Effect::new(move |_| {
        if let Some(id) = item.get() {
            if !app
                .item_runs
                .with_untracked(|pages| pages.contains_key(&id))
            {
                app.load_item_runs(&backend, &id, false);
            }
        }
    });
    let counts = Memo::new(move |_| RunCounts::of(&runs_of(app, review), &waiting(review)));
    let toggle = move || review.agents_unfolded.update(|open| *open = !*open);
    Stack::horizontal((
        dyn_container(
            move || review.agents_unfolded.get(),
            |open| {
                sidebar_glyph(
                    if open {
                        SidebarIcon::ChevronDown
                    } else {
                        SidebarIcon::ChevronRight
                    },
                    theme::FG_SUBTLE,
                )
                .style(|s| s.size(10.0, 10.0))
            },
        )
        .style(|s| s.margin_right(6.0)),
        Label::derived(|| "AGENTS".to_string()).style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG_SUBTLE)
                .selectable(false)
                .flex_grow(1.0_f32)
        }),
        Empty::new().style(move |s| {
            let counts = counts.get();
            let color = if counts.running > 0 {
                status_color(AgentRunStatus::Running)
            } else {
                status_color(AgentRunStatus::Queued)
            };
            s.size(7.0, 7.0)
                .border_radius(4.0)
                .background(color)
                .margin_right(7.0)
                .flex_shrink(0.0_f32)
                .apply_if(!counts.active(), |s| s.hide())
        }),
        Label::derived(move || counts.get().label()).style(move |s| {
            let counts = counts.get();
            s.font_size(theme::TEXT_BADGE)
                .color(if !counts.active() && counts.last_failed {
                    theme::CLOSED
                } else {
                    theme::FG_MUTED
                })
                .selectable(false)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| toggle())
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            toggle();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .debug_name("agents header")
    .style(|s| {
        clickable(s)
            .items_center()
            .width_full()
            .height(22.0)
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
    })
}

/// A key for an entry that changes when what its row shows does: a run's
/// row follows its run on its own, and a review's is built again when an
/// attempt joins it or finishes.
fn entry_key(entry: &AgentRunEntry) -> String {
    match entry {
        AgentRunEntry::Run(run) => format!("run:{}", run.id),
        AgentRunEntry::ReviewGroup(group) => {
            let (status, done) = group
                .summary
                .as_ref()
                .map(|summary| {
                    (
                        summary.group.status.name().to_string(),
                        summary
                            .attempts
                            .iter()
                            .filter(|attempt| attempt.phase.is_terminal())
                            .count(),
                    )
                })
                .unwrap_or_default();
            let runs = group
                .runs
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>()
                .join(",");
            format!("review:{}:{runs}:{status}:{done}", group.key)
        }
    }
}

/// The Agents section, over REVIEW HISTORY: its header, and the list
/// under it while unfolded. Absent until the item has had a run.
pub fn section(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Stack::vertical((section_header(app, review, backend.clone()), list(app, review, backend)))
        .debug_name("agents section")
        .style(move |s| {
            s.flex_col()
                .width_full()
                .gap(10.0)
                .padding_top(18.0)
                .padding_bottom(18.0)
                .border_top(1.0)
                .border_color(theme::BORDER_MUTED)
                .apply_if(!present(app, review), |s| s.hide())
        })
}

/// The section's list while it is unfolded.
fn list(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // A run to bring into view: its row unfolds, opens and, in a review,
    // its group opens too. The row scrolls itself to the top once laid out.
    let reveal_backend = backend.clone();
    Effect::new(move |_| {
        let Some(run_id) = review.pending_run_reveal.get() else {
            return;
        };
        let Some(item_id) = item_id(review) else {
            return;
        };
        let Some(run) = app.agent_run(&run_id) else {
            return;
        };
        if run.item_id.as_deref() != Some(item_id.as_str()) {
            return;
        }
        floem::reactive::Effect::batch(|| {
            if !review.agents_unfolded.get_untracked() {
                review.agents_unfolded.set(true);
            }
            if !review
                .agents_open_runs
                .with_untracked(|open| open.contains(&run_id))
            {
                review.agents_open_runs.update(|open| {
                    open.insert(run_id.clone());
                });
                app.set_agent_run_events_loading(&run_id);
                reveal_backend.load_agent_run_detail(&run_id);
            }
            if let Some(attempt) = &run.review_attempt_id {
                let groups = review_groups(app, &item_id);
                let key = super::model::review_group_for_attempt(&groups, attempt)
                    .map(|summary| summary.group.id.clone())
                    .unwrap_or_else(|| format!("attempt:{attempt}"));
                if app
                    .collapsed_agent_review_groups
                    .with_untracked(|collapsed| collapsed.contains(&key))
                {
                    app.collapsed_agent_review_groups.update(|collapsed| {
                        collapsed.remove(&key);
                    });
                }
            }
        });
    });

    let entries = Memo::new(move |_| {
        let Some(item_id) = item_id(review) else {
            return Vec::new();
        };
        let groups = review_groups(app, &item_id);
        let runs = app
            .agent_runs
            .with(|runs| item_runs(runs, &item_id, &groups));
        group_review_runs(&runs, &groups)
    });
    let row_backend = backend.clone();
    Stack::vertical((
        dyn_stack(
            move || entries.get(),
            entry_key,
            move |entry| match entry {
                AgentRunEntry::Run(run) => {
                    run_row(app, review, row_backend.clone(), run).into_any()
                }
                AgentRunEntry::ReviewGroup(group) => {
                    review_group_row(app, review, row_backend.clone(), group).into_any()
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
        older(app, review, backend),
    ))
    .debug_name("agents list")
    .style(move |s| {
        let shown = review.agents_unfolded.get() && entries.with(|e| !e.is_empty());
        s.flex_col()
            .width_full()
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::RADIUS)
            .background(theme::CANVAS)
            .apply_if(!shown, |s| s.hide())
    })
}

/// Load older runs, while the item has more.
fn older(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let page =
        move || item_id(review).and_then(|id| app.item_runs.with(|pages| pages.get(&id).copied()));
    dyn_container(
        move || page().map(|page| (page.has_more, page.loading)),
        move |page| match page {
            Some((true, loading)) => {
                let backend = backend.clone();
                text_action(
                    move || {
                        if loading {
                            "Loading…".to_string()
                        } else {
                            "Load older runs".to_string()
                        }
                    },
                    false,
                    move || {
                        if let (false, Some(id)) = (loading, item_id(review)) {
                            app.load_item_runs(&backend, &id, true);
                        }
                    },
                )
                .style(|s| s.margin(8.0))
                .into_any()
            }
            _ => Empty::new().into_any(),
        },
    )
    .debug_name("agents older")
    .style(|s| s.width_full().justify_center())
}

/// Open a run in the list, or close it. Opening asks for what it did.
fn toggle_run(app: AppState, review: ReviewState, backend: &Backend, id: &str) {
    let open = review
        .agents_open_runs
        .with_untracked(|open| open.contains(id));
    review.agents_open_runs.update(|runs| {
        if open {
            runs.remove(id);
        } else {
            runs.insert(id.to_string());
        }
    });
    if !open {
        app.set_agent_run_events_loading(id);
        backend.load_agent_run_detail(id);
    }
}

fn run_row(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    initial: AgentRunSummary,
) -> impl IntoView {
    let id = initial.id.clone();
    let run = {
        let id = id.clone();
        Memo::new(move |_| app.agent_run(&id).unwrap_or_else(|| initial.clone()))
    };
    let open = {
        let id = id.clone();
        Memo::new(move |_| review.agents_open_runs.with(|open| open.contains(&id)))
    };
    // An open run whose detail was never asked for, one reopened with its
    // item, asks for it as it is drawn.
    if open.get_untracked()
        && app
            .agent_run_events
            .with_untracked(|all| !all.contains_key(&id))
    {
        app.set_agent_run_events_loading(&id);
        backend.load_agent_run_detail(&id);
    }
    let header_backend = backend.clone();
    let click_backend = backend.clone();
    let key_backend = backend.clone();
    let (click_id, key_id) = (id.clone(), id.clone());
    let header = dyn_container(
        move || run.get(),
        move |run| header(app, review, header_backend.clone(), run).into_any(),
    )
    .on_event_stop(el::Click, move |_, _| {
        toggle_run(app, review, &click_backend, &click_id)
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            toggle_run(app, review, &key_backend, &key_id);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(move |s| {
        let base = clickable(s)
            .width_full()
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .hover(|s| s.background(theme::CANVAS_HOVER));
        if open.get() {
            base.background(theme::ACCENT_SUBTLE)
        } else {
            base
        }
    })
    .debug_name("agent-run-header");
    let body = dyn_container(
        move || open.get(),
        move |open| {
            if open {
                detail::inline(
                    app,
                    backend.clone(),
                    run.get_untracked(),
                    OPEN_RUN_MAX_HEIGHT,
                )
                .into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| s.width_full());

    let row = Stack::vertical((header, body))
        .debug_name(format!("agent-run:{id}"))
        .style(|s| {
            s.flex_col()
                .width_full()
                .border_bottom(1.0)
                .border_color(theme::BORDER_MUTED)
        });
    // Brought into view, it scrolls the page to put itself at the top: a
    // rectangle taller than any window shows its top edge.
    let row_id = row.view_id();
    row.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if review
            .pending_run_reveal
            .with_untracked(|pending| pending.as_deref() == Some(id.as_str()))
        {
            review.pending_run_reveal.set(None);
            row_id.scroll_to(Some(Rect::new(0.0, 0.0, 1.0, 100_000.0)));
        }
    })
}

/// A run's row: its status and what ran it, what it was asked, what it
/// is doing or how it ended, and what can be done with it.
fn header(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    run: AgentRunSummary,
) -> impl IntoView {
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
            AgentRunStatus::Failed | AgentRunStatus::Stopped | AgentRunStatus::Interrupted => run
                .terminal_display()
                .unwrap_or_else(|| run.status.label().to_string()),
        });
    let elapsed_run = run.clone();
    let queue = queue_position(app, &run.id);
    let status = run.status;
    let purpose = run.purpose.label();
    Stack::vertical((
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
            }),
            Label::derived(move || {
                let elapsed = elapsed_label(&elapsed_run, app.now.get());
                if status == AgentRunStatus::Queued {
                    let queued_for = elapsed.strip_prefix("Queued ").unwrap_or(&elapsed);
                    match queue {
                        Some(position) => format!("Queue #{position} · {queued_for}"),
                        None => format!("Queued · {queued_for}"),
                    }
                } else {
                    format!("{} · {elapsed}", status.label())
                }
            })
            .style(move |s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(theme::weight_emphasis())
                    .color(color)
                    .flex_shrink(0.0_f32)
                    .selectable(false)
            }),
        ))
        .style(|s| s.items_center().width_full().gap(7.0)),
        Label::derived(move || subject.clone()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG)
                .text_ellipsis()
                .width_full()
                .selectable(false)
        }),
        Label::derived(move || activity.clone()).style(move |s| {
            s.font_size(theme::TEXT_BADGE)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(if status == AgentRunStatus::Failed {
                    theme::CLOSED
                } else {
                    theme::FG_SUBTLE
                })
                .text_ellipsis()
                .width_full()
                .selectable(false)
        }),
        Stack::horizontal((
            Label::derived(move || purpose.to_string()).style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .color(theme::FG_SUBTLE)
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
                    .selectable(false)
            }),
            actions(review, backend, run),
        ))
        .style(|s| s.items_center().width_full().height(27.0)),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .gap(6.0)
            .padding_vert(10.0)
            .padding_left(14.0)
            .padding_right(10.0)
    })
}

/// Show in thread, for a run that has one, then Cancel, Stop or Retry as
/// the run allows.
fn actions(review: ReviewState, backend: Rc<Backend>, run: AgentRunSummary) -> impl IntoView {
    let thread = match run.thread_id.clone() {
        Some(thread_id) => text_action(
            || "Show in thread".to_string(),
            true,
            move || {
                review
                    .focus_card
                    .set(Some(CardId::Thread(thread_id.clone())))
            },
        )
        .debug_name("agent-run show in thread")
        .into_any(),
        None => Empty::new().into_any(),
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
    Stack::horizontal((thread, control)).style(|s| s.items_center().gap(2.0).flex_shrink(0.0_f32))
}

fn title_case_status(status: &str) -> String {
    let mut chars = status.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// A review's row, `Review #n · sha · against base` and how far it has
/// got, over its attempts, with the way to the snapshot it read. It folds.
fn review_group_row(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    group: AgentReviewRunGroup,
) -> impl IntoView {
    let key = group.key.clone();
    let count = group.runs.len();
    let (title, status) = match group.summary.as_ref() {
        Some(summary) => {
            let title = format!(
                "Review #{} · {} · against {}",
                summary.group.number,
                summary.snapshot.head_label(),
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
    let collapsed = {
        let key = key.clone();
        Memo::new(move |_| app.collapsed_agent_review_groups.with(|c| c.contains(&key)))
    };
    let toggle = {
        let key = key.clone();
        move || {
            app.collapsed_agent_review_groups.update(|c| {
                if !c.insert(key.clone()) {
                    c.remove(&key);
                }
            })
        }
    };
    let key_toggle = toggle.clone();
    // The snapshot the review read, as REVIEW HISTORY's card opens it.
    let snapshot = match group.runs.first().and_then(|run| {
        Some((run.item_id.clone()?, run.review_attempt_id.clone()?))
    }) {
        Some((item_id, attempt_id)) => {
            let backend = backend.clone();
            text_action(
                || "Open snapshot".to_string(),
                false,
                move || {
                    let navigation_id = backend.view_review_attempt_owner(&item_id, &attempt_id);
                    app.begin_agent_review_navigation(
                        review,
                        item_id.clone(),
                        attempt_id.clone(),
                        navigation_id,
                    );
                },
            )
            .debug_name("agent-review-group open snapshot")
            .into_any()
        }
        None => Empty::new().into_any(),
    };
    let header = Stack::horizontal((
        dyn_container(
            move || collapsed.get(),
            |collapsed| {
                sidebar_glyph(
                    if collapsed {
                        SidebarIcon::ChevronRight
                    } else {
                        SidebarIcon::ChevronDown
                    },
                    theme::FG_SUBTLE,
                )
                .style(|s| s.size(10.0, 10.0))
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
            }),
            Label::derived(move || status.clone()).style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG_MUTED)
                    .selectable(false)
            }),
        ))
        .style(|s| s.flex_col().gap(2.0).min_width(0.0).flex_grow(1.0_f32)),
        snapshot,
    ))
    .on_event_stop(el::Click, move |_, _| toggle())
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            key_toggle();
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
    .debug_name(format!("agent-review-group:{key}"));
    let runs = group.runs.clone();
    let children = dyn_container(
        move || collapsed.get(),
        move |collapsed| {
            if collapsed {
                return Empty::new().into_any();
            }
            let backend = backend.clone();
            let runs = runs.clone();
            dyn_stack(
                move || runs.clone(),
                |run| run.id.clone(),
                move |run| run_row(app, review, backend.clone(), run),
            )
            .style(|s| {
                s.flex_col()
                    .width_full()
                    .padding_left(12.0)
                    .background(theme::CANVAS_SUBTLE)
            })
            .into_any()
        },
    );
    Stack::vertical((header, children)).style(|s| s.flex_col().width_full())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use floem::headless::HeadlessHarness;

    use crate::review::header::tests::{named, pull_request_harness, rect};

    fn at(s: &str) -> jiff::Timestamp {
        s.parse().unwrap()
    }

    /// A run on the harness's pull request, `pr`, asked in thread `t-{id}`.
    fn record(id: &str, status: perch_github::RunStatus, minute: u32) -> perch_github::RunRecord {
        let when = at(&format!("2026-09-19T10:{minute:02}:00Z"));
        perch_github::RunRecord {
            id: id.into(),
            owner: perch_github::RunOwner::Discussion {
                item_id: "pr".into(),
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

    fn add(app: AppState, records: &[perch_github::RunRecord]) {
        app.merge_agent_runs(records.iter().map(perch_github::RunSummary::from), false);
    }

    fn settle(harness: &mut HeadlessHarness) {
        harness.rebuild();
        harness.paint();
        harness.rebuild();
        harness.paint();
    }

    fn click(harness: &mut HeadlessHarness, name: &str) {
        let at = rect(harness, name).center();
        harness.click(at.x, at.y);
        settle(harness);
    }

    fn shown(harness: &HeadlessHarness, name: &str) -> bool {
        named(harness.root_id(), name).is_some_and(|id| id.get_visual_rect_no_clip().height() > 0.0)
    }

    /// The section is absent until the item has had a run. Its header
    /// says what the runs are doing, and folds and unfolds the list,
    /// working first, then finished. It sits over REVIEW HISTORY, not in
    /// the summary's rows.
    #[test]
    fn the_agents_section_comes_with_a_run_and_its_header_unfolds_the_list() {
        let (mut harness, app, review) = pull_request_harness();
        assert!(!shown(&harness, "agents section"), "a section with no runs");
        assert!(
            app.item_runs
                .with(|pages| pages.get("pr").is_some_and(|page| page.loading)),
            "the item's runs were not asked for"
        );

        add(
            app,
            &[
                record("a", perch_github::RunStatus::Completed, 1),
                record("b", perch_github::RunStatus::Running, 2),
            ],
        );
        settle(&mut harness);
        assert!(shown(&harness, "agents section"));
        assert!(named(harness.root_id(), "summary row Agents").is_none());
        assert!(named(harness.root_id(), "\"1 running · 1 done\"").is_some());
        assert!(
            !shown(&harness, "agents list"),
            "unfolded before it was asked"
        );

        click(&mut harness, "agents header");
        assert!(review.agents_unfolded.get());
        let list = rect(&harness, "agents list");
        assert!(list.height() > 0.0);
        assert!(
            list.y0 >= rect(&harness, "card:description").y1 - 0.5,
            "the list is over the cards"
        );
        let (a, b) = (rect(&harness, "agent-run:a"), rect(&harness, "agent-run:b"));
        assert!(b.y1 <= a.y0, "the running one is first");

        click(&mut harness, "agents header");
        assert!(!review.agents_unfolded.get());
        assert!(!shown(&harness, "agents list"));
    }

    /// A run opens in place to what it did, no taller than the cap, and
    /// more than one can be open. Its header closes it again. Show in
    /// thread flashes the run's thread card.
    #[test]
    fn a_run_opens_in_place_capped_and_shows_its_thread() {
        let (mut harness, app, review) = pull_request_harness();
        let mut long = record("a", perch_github::RunStatus::Completed, 1);
        long.final_result = Some("A line of the answer.\n\n".repeat(200));
        add(
            app,
            &[
                long.clone(),
                record("b", perch_github::RunStatus::Failed, 2),
            ],
        );
        app.set_agent_run_detail(&long, Vec::new(), false);
        review.agents_unfolded.set(true);
        settle(&mut harness);

        // The list is down the page, so a header is brought into view
        // before it is clicked.
        let header = |harness: &mut HeadlessHarness, id: &str| {
            let row = named(harness.root_id(), &format!("agent-run:{id}")).unwrap();
            let header = named(row, "agent-run-header").unwrap();
            header.scroll_to(None);
            settle(harness);
            header.get_visual_rect_no_clip()
        };
        let a = header(&mut harness, "a");
        harness.click(a.x0 + 20.0, a.y0 + 14.0);
        settle(&mut harness);
        assert!(review.agents_open_runs.with(|open| open.contains("a")));
        let body = rect(&harness, "agent-run-inline");
        assert!(body.height() > 100.0, "the run did not open: {body:?}");
        assert!(
            body.height() <= OPEN_RUN_MAX_HEIGHT + 0.5,
            "the open run is taller than the cap: {body:?}"
        );

        let b = header(&mut harness, "b");
        harness.click(b.x0 + 20.0, b.y0 + 14.0);
        settle(&mut harness);
        assert!(review
            .agents_open_runs
            .with(|open| open.contains("a") && open.contains("b")));

        let a = header(&mut harness, "a");
        harness.click(a.x0 + 20.0, a.y0 + 14.0);
        settle(&mut harness);
        assert!(review
            .agents_open_runs
            .with(|open| !open.contains("a") && open.contains("b")));

        let row = named(harness.root_id(), "agent-run:a").unwrap();
        let show = named(row, "agent-run show in thread").unwrap();
        show.scroll_to(None);
        settle(&mut harness);
        let show = show.get_visual_rect_no_clip();
        harness.click(show.center().x, show.center().y);
        settle(&mut harness);
        assert_eq!(review.focus_card.get(), Some(CardId::Thread("t-a".into())));
        assert!(
            review.agents_open_runs.with(|open| !open.contains("a")),
            "Show in thread opened the run too"
        );
    }

    /// Older runs load a page at a time while the item has more.
    #[test]
    fn older_runs_load_while_the_item_has_more() {
        let (mut harness, app, review) = pull_request_harness();
        add(app, &[record("a", perch_github::RunStatus::Completed, 1)]);
        app.item_runs.update(|pages| {
            pages.insert(
                "pr".into(),
                crate::state::ItemRunsPage {
                    has_more: true,
                    loading: false,
                },
            );
        });
        review.agents_unfolded.set(true);
        settle(&mut harness);
        assert!(named(harness.root_id(), "\"Load older runs\"").is_some());
        click(&mut harness, "\"Load older runs\"");
        assert!(app.item_runs.with(|pages| pages["pr"].loading));
        assert!(named(harness.root_id(), "\"Loading…\"").is_some());
    }

    /// A review's attempts are under a row for the review, which folds.
    #[test]
    fn a_reviews_attempts_are_under_its_row() {
        let (mut harness, app, review) = pull_request_harness();
        let attempt = |id: &str, run: &str, minute| perch_github::RunRecord {
            owner: perch_github::RunOwner::Review {
                item_id: "pr".into(),
                reviewer_run_id: id.into(),
            },
            purpose: perch_github::comments::RunPurpose::WorktreeReview,
            ..record(run, perch_github::RunStatus::Completed, minute)
        };
        add(
            app,
            &[
                attempt("attempt-claude", "claude-run", 1),
                attempt("attempt-codex", "codex-run", 2),
                record("q", perch_github::RunStatus::Completed, 3),
            ],
        );
        app.review_groups.update(|all| {
            all.insert(
                "pr".into(),
                Arc::new(vec![super::super::model::tests::review_summary(
                    "group-3",
                    &["attempt-claude", "attempt-codex"],
                )]),
            );
        });
        review.agents_unfolded.set(true);
        settle(&mut harness);
        let group = named(harness.root_id(), "agent-review-group:group-3").expect("no review row");
        assert!(named(harness.root_id(), "\"Review #3 · head · against main\"").is_some());
        let group = group.get_visual_rect_no_clip();
        for run in ["claude-run", "codex-run"] {
            assert!(rect(&harness, &format!("agent-run:{run}")).y0 >= group.y1 - 0.5);
        }

        harness.click(group.center().x, group.center().y);
        settle(&mut harness);
        assert!(
            !shown(&harness, "agent-run:claude-run"),
            "the review did not fold"
        );
        assert!(shown(&harness, "agent-run:q"));
    }

    /// While a review runs, the bar's Review says so, and Show in its
    /// popover goes to it in the Agents section.
    #[test]
    fn review_goes_to_a_running_review_in_the_agents_row() {
        let (mut harness, app, review) = pull_request_harness();
        let attempt = |id: &str, run: &str, minute| perch_github::RunRecord {
            owner: perch_github::RunOwner::Review {
                item_id: "pr".into(),
                reviewer_run_id: id.into(),
            },
            purpose: perch_github::comments::RunPurpose::WorktreeReview,
            ..record(run, perch_github::RunStatus::Running, minute)
        };
        add(app, &[attempt("attempt-claude", "claude-run", 1)]);
        let mut summary =
            super::super::model::tests::review_summary("group-3", &["attempt-claude"]);
        summary.group.status = perch_github::ReviewGroupStatus::Reconciling;
        app.review_groups.update(|all| {
            all.insert("pr".into(), Arc::new(vec![summary]));
        });
        review.show_tab(crate::review::model::ItemTab::Code);
        settle(&mut harness);

        assert!(named(harness.root_id(), "Reviewing \u{b7} 1 of 1 complete").is_some());
        click(&mut harness, "review-control");
        assert!(review.review_open.get());
        click(&mut harness, "review-show-agents");
        assert!(!review.review_open.get(), "Show left the popover open");
        assert_eq!(
            review.tab.get(),
            crate::review::model::ItemTab::Conversation
        );
        assert!(review.agents_unfolded.get());
        assert!(review
            .agents_open_runs
            .with(|open| open.contains("claude-run")));
    }

    /// A run asked for from elsewhere comes into view: the section unfolds,
    /// the run opens, and the ask is done with once the row is laid out.
    #[test]
    fn a_run_asked_for_unfolds_the_row_and_opens() {
        let (mut harness, app, review) = pull_request_harness();
        add(
            app,
            &[
                record("a", perch_github::RunStatus::Completed, 1),
                record("b", perch_github::RunStatus::Completed, 2),
            ],
        );
        settle(&mut harness);
        assert!(!review.agents_unfolded.get());

        review.pending_run_reveal.set(Some("a".into()));
        settle(&mut harness);
        assert!(review.agents_unfolded.get());
        assert!(review.agents_open_runs.with(|open| open.contains("a")));
        assert!(shown(&harness, "agent-run-inline"));
        assert_eq!(review.pending_run_reveal.get(), None);
    }
}

