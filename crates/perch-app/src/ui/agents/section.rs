//! An item's runs and reviews, on its Conversation: the Agents section,
//! whose header says what they are doing and folds the list under it.
//!
//! The list is what is working first and then what finished, newest
//! first. A question is a row with a way to its thread, where its answer
//! is; it opens in place to what it did. A review is a row that opens to
//! what it found, each finding a way to its thread, and to its reviewers,
//! each with its raw output behind a word (`review_row`). A review is
//! listed whether or not its runs have loaded.

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
use crate::timefmt;
use crate::ui::widgets::{clickable, sidebar_glyph};

use super::model::{
    elapsed_label, group_review_runs, item_runs, with_every_review, AgentRetryTarget,
    AgentRunEntry, AgentRunStatus, AgentRunSummary, RunCounts,
};
use super::{detail, live_dot, queue_position, status_color, status_mark, text_action};

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

/// Whether the open item has had a run or a review. Tracked.
fn present(app: AppState, review: ReviewState) -> bool {
    let Some(item_id) = item_id(review) else {
        return false;
    };
    !review_groups(app, &item_id).is_empty()
        || app.agent_runs.with(|runs| {
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
        crate::review::cards::section_title("Agents"),
        dyn_container(
            move || {
                let counts = counts.get();
                (counts.active(), counts.running > 0)
            },
            move |(active, running)| match (active, running) {
                (true, true) => live_dot(app, 7.0).into_any(),
                (true, false) => Empty::new()
                    .style(|s| {
                        s.size(7.0, 7.0)
                            .border_radius(4.0)
                            .background(status_color(AgentRunStatus::Queued)())
                    })
                    .into_any(),
                _ => Empty::new().into_any(),
            },
        )
        .style(|s| s.flex_shrink(0.0_f32)),
        // While anything works, its busiest state leads, in that state's
        // colour, and the rest follow quietly.
        Label::derived(move || {
            let label = counts.get().label();
            match counts.get().active() {
                true => label
                    .split(" \u{b7} ")
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                false => label,
            }
        })
        .style(move |s| {
            let counts = counts.get();
            s.font_size(theme::TEXT_META)
                .color(if counts.running > 0 {
                    theme::running()
                } else if counts.active() {
                    theme::notice_fg()
                } else if counts.last_failed {
                    theme::closed()
                } else {
                    theme::fg_muted()
                })
                .flex_shrink(0.0_f32)
                .selectable(false)
        }),
        Label::derived(move || {
            let label = counts.get().label();
            match (counts.get().active(), label.split_once(" \u{b7} ")) {
                (true, Some((_, rest))) => format!("\u{b7} {rest}"),
                _ => String::new(),
            }
        })
        .style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_subtle())
                .text_ellipsis()
                .min_width(0.0)
                .selectable(false)
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        dyn_container(
            move || review.agents_unfolded.get(),
            |open| {
                sidebar_glyph(
                    if open {
                        SidebarIcon::ChevronUp
                    } else {
                        SidebarIcon::ChevronDown
                    },
                    theme::fg_subtle,
                )
                .style(|s| s.size(12.0, 12.0))
            },
        ),
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
        crate::review::cards::section_header_style(clickable(s))
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
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
            // The counts too, which its row shows as they were when built.
            let counts = group
                .summary
                .as_ref()
                .map(|summary| {
                    let g = &summary.group;
                    format!(
                        "{}/{}/{}/{}",
                        g.new_findings, g.still_present, g.verified_fixed, g.not_verified
                    )
                })
                .unwrap_or_default();
            format!("review:{}:{runs}:{status}:{done}:{counts}", group.key)
        }
    }
}

/// The Agents section: its header, and the list under it while
/// unfolded. Absent until the item has had a run or a review.
pub fn section(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Stack::vertical((
        section_header(app, review, backend.clone()),
        list(app, review, backend),
    ))
    .debug_name("agents section")
    // Perch's own runs, so on Perch's grey, as the reader's threads are.
    .style(move |s| {
        crate::review::cards::perch_surface(s.flex_col().width_full())
            .margin_top(theme::CONVERSATION_ENTRY_PADDING)
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
        with_every_review(group_review_runs(&runs, &groups), &groups)
    });
    let row_backend = backend.clone();
    Stack::vertical((
        dyn_stack(
            move || entries.get(),
            entry_key,
            move |entry| match entry {
                AgentRunEntry::Run(run) => {
                    run_row(app, review, row_backend.clone(), run, false).into_any()
                }
                AgentRunEntry::ReviewGroup(group) => {
                    super::review_row::review_row(app, review, row_backend.clone(), group)
                        .into_any()
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
        older(app, review, backend),
    ))
    .debug_name("agents list")
    .style(move |s| {
        let shown = review.agents_unfolded.get() && entries.with(|e| !e.is_empty());
        s.flex_col().width_full().apply_if(!shown, |s| s.hide())
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
pub(super) fn toggle_run(app: AppState, review: ReviewState, backend: &Backend, id: &str) {
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

/// A run's row, and what it did under it while open, which a click on
/// it opens: a question's, or a reviewer's line under its review, whose
/// raw output it opens to, since the review's findings say what it found.
pub(super) fn run_row(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    initial: AgentRunSummary,
    reviewer: bool,
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
    let hovered = RwSignal::new(false);
    let header = dyn_container(
        move || run.get(),
        move |run| {
            if reviewer {
                reviewer_header(app, review, header_backend.clone(), run, open, hovered).into_any()
            } else {
                header(app, review, header_backend.clone(), run, hovered).into_any()
            }
        },
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
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()));
        if reviewer {
            return base
                .border_radius(theme::RADIUS)
                .hover(|s| s.background(theme::canvas_hover()));
        }
        let base = base.hover(|s| s.background(theme::canvas_hover()));
        if open.get() {
            base.background(theme::accent_subtle())
        } else {
            base
        }
    })
    .debug_name("agent-run-header");
    let header = crate::review::cards::track_hover(header, hovered);
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
    .style(move |s| s.width_full().apply_if(reviewer, |s| s.margin_top(6.0)));

    let row = Stack::vertical((header, body))
        .debug_name(format!("agent-run:{id}"))
        .style(move |s| {
            let s = s.flex_col().width_full();
            if reviewer {
                s
            } else {
                s.border_top(1.0).border_color(theme::border_muted())
            }
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

/// A question's row: its mark, what it was asked, and under that what
/// ran it and why, and while it works or once it failed, what it is doing
/// or how it ended; then how long it has worked or when it ran, and, under
/// the pointer, what can be done with it. Its answer is in its thread,
/// which Show in thread opens.
fn header(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    run: AgentRunSummary,
    hovered: RwSignal<bool>,
) -> impl IntoView {
    let color = status_color(run.status);
    let status = run.status;
    let failed = matches!(
        status,
        AgentRunStatus::Failed | AgentRunStatus::Stopped | AgentRunStatus::Interrupted
    );
    let subject = run.subject();
    let about = format!(
        "{} \u{b7} {}",
        crate::review::threads::agent_name(&run.backend, &run.model, &run.effort),
        run.purpose.label().to_lowercase()
    );
    let activity = if status.is_active() {
        Some(run.latest_activity.clone().unwrap_or_else(|| match status {
            AgentRunStatus::Queued => "Waiting for an execution slot".to_string(),
            AgentRunStatus::Starting => "Starting the agent".to_string(),
            AgentRunStatus::Stopping => "Stopping the process tree…".to_string(),
            _ => "Working…".to_string(),
        }))
    } else if failed {
        Some(
            run.terminal_display()
                .unwrap_or_else(|| status.label().to_string()),
        )
    } else {
        None
    };
    let when = run_when(app, &run);
    Stack::horizontal((
        status_mark(app, status),
        Stack::vertical((
            Label::derived(move || subject.clone()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::fg())
                    .text_ellipsis()
                    .min_width(0.0)
                    .selectable(false)
            }),
            Label::derived(move || about.clone()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .text_ellipsis()
                    .min_width(0.0)
                    .selectable(false)
            }),
            match activity {
                Some(activity) => Label::derived(move || activity.clone())
                    .style(move |s| {
                        let s = s
                            .font_size(theme::TEXT_BADGE)
                            .color(if failed {
                                theme::closed()
                            } else {
                                theme::fg_muted()
                            })
                            .text_ellipsis()
                            .min_width(0.0)
                            .selectable(false);
                        if failed {
                            s
                        } else {
                            s.font_family(theme::CODE_FONT_STACK.to_string())
                        }
                    })
                    .into_any(),
                None => Empty::new().into_any(),
            },
        ))
        .style(|s| {
            s.flex_col()
                .gap(3.0)
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
        Label::derived(when)
            .debug_name("agent-run-when")
            .style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(if status.is_active() {
                        color()
                    } else {
                        theme::fg_subtle()
                    })
                    .flex_shrink(0.0_f32)
                    .selectable(false)
            }),
        actions(review, backend, run, hovered),
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .gap(12.0)
            .min_height(48.0)
            .padding_vert(10.0)
            .padding_horiz(theme::CARD_PADDING)
    })
}

/// A reviewer's line under its review, in a box of its own: a dot for how
/// it stands, what ran it, and what it is doing or how it went; under the
/// pointer, Stop or Retry; and a chevron for its raw output, which the
/// whole line opens under it.
fn reviewer_header(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    run: AgentRunSummary,
    open: Memo<bool>,
    hovered: RwSignal<bool>,
) -> impl IntoView {
    let color = status_color(run.status);
    let status = run.status;
    let name = crate::review::threads::agent_name(&run.backend, &run.model, &run.effort);
    let cost = run
        .cost_usd
        .map(|cost| format!(" \u{b7} ${cost:.2}"))
        .unwrap_or_default();
    let activity = run.latest_activity.clone().filter(|_| status.is_active());
    let live = activity.is_some();
    let elapsed_run = run.clone();
    let dot = if matches!(status, AgentRunStatus::Starting | AgentRunStatus::Running) {
        live_dot(app, 6.0).into_any()
    } else {
        Empty::new()
            .style(move |s| {
                s.size(6.0, 6.0)
                    .border_radius(3.0)
                    .background(color())
                    .flex_shrink(0.0_f32)
            })
            .into_any()
    };
    Stack::horizontal((
        dot,
        Label::derived(move || name.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg())
                .text_ellipsis()
                .min_width(0.0)
                .flex_shrink(1.0_f32)
                .selectable(false)
        }),
        Label::derived(move || match &activity {
            Some(activity) => activity.clone(),
            None => format!(
                "{} \u{b7} {}{cost}",
                status.label(),
                elapsed_label(&elapsed_run, app.now.get())
            ),
        })
        .debug_name("agent-reviewer-status")
        .style(move |s| {
            let s = s
                .font_size(theme::TEXT_BADGE)
                .color(match status {
                    AgentRunStatus::Completed => theme::fg_subtle(),
                    _ if live => theme::fg_muted(),
                    _ => color(),
                })
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .selectable(false);
            if live {
                s.font_family(theme::CODE_FONT_STACK.to_string())
            } else {
                s
            }
        }),
        actions(review, backend, run, hovered),
        crate::review::cards::tip(
            dyn_container(
                move || open.get(),
                |open| {
                    sidebar_glyph(
                        if open {
                            SidebarIcon::ChevronDown
                        } else {
                            SidebarIcon::ChevronRight
                        },
                        theme::fg_subtle,
                    )
                    .style(|s| s.size(10.0, 10.0))
                },
            )
            .debug_name("agent-reviewer raw output")
            .style(|s| {
                s.size(16.0, 16.0)
                    .items_center()
                    .justify_center()
                    .flex_shrink(0.0_f32)
            }),
            "Raw output".to_string(),
        ),
    ))
    .style(move |s| {
        s.items_center()
            .width_full()
            .gap(8.0)
            .min_height(32.0)
            .padding_horiz(8.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
            .apply_if(open.get(), |s| s.background(theme::accent_subtle()))
    })
}

/// How a run stands or when it ran: how long while it works, in the
/// colour of its state, its place while queued, and else only when.
/// Kept current by the clock.
fn run_when(app: AppState, run: &AgentRunSummary) -> impl Fn() -> String + 'static {
    let run = run.clone();
    let queue = queue_position(app, &run.id);
    move || {
        let now = app.now.get();
        match run.status {
            AgentRunStatus::Queued => {
                let elapsed = elapsed_label(&run, now);
                let queued_for = elapsed
                    .strip_prefix("Queued ")
                    .unwrap_or(&elapsed)
                    .to_string();
                match queue {
                    Some(position) => format!("Queue #{position} \u{b7} {queued_for}"),
                    None => format!("Queued \u{b7} {queued_for}"),
                }
            }
            AgentRunStatus::Stopping => format!("Stopping \u{b7} {}", elapsed_label(&run, now)),
            status if status.is_active() => elapsed_label(&run, now),
            _ => timefmt::short(run.ran_at(), now),
        }
    }
}

/// Show in thread, for a run that has one, then Cancel, Stop or Retry as
/// the run allows: glyphs, shown while the row is under the pointer.
fn actions(
    review: ReviewState,
    backend: Rc<Backend>,
    run: AgentRunSummary,
    hovered: RwSignal<bool>,
) -> impl IntoView {
    use crate::review::cards::hover_action;
    let thread = match run.thread_id.clone() {
        Some(thread_id) => hover_action(
            hovered,
            SidebarIcon::Thread,
            "Show in thread",
            "agent-run show in thread",
            Rc::new(move || {
                review
                    .focus_card
                    .set(Some(CardId::Thread(thread_id.clone())))
            }),
        )
        .into_any(),
        None => Empty::new().into_any(),
    };
    let control = match run.status {
        AgentRunStatus::Queued => {
            let id = run.id.clone();
            hover_action(
                hovered,
                SidebarIcon::Close,
                "Cancel",
                "agent-run cancel",
                Rc::new(move || backend.cancel_run(&id)),
            )
            .into_any()
        }
        status if status.may_stop() => {
            let id = run.id.clone();
            hover_action(
                hovered,
                SidebarIcon::Stop,
                "Stop",
                "agent-run stop",
                Rc::new(move || backend.cancel_run(&id)),
            )
            .into_any()
        }
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
    };
    Stack::horizontal((thread, control)).style(|s| s.items_center().gap(2.0).flex_shrink(0.0_f32))
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
        app.merge_agent_runs(records.iter().map(perch_github::RunSummary::from));
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
    /// working first, then finished. It sits over Reviews, not in
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
        assert!(named(harness.root_id(), "\"1 running\"").is_some());
        assert!(named(harness.root_id(), "\"· 1 done\"").is_some());
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

        // Show in thread is drawn while the row is under the pointer.
        let a = header(&mut harness, "a");
        harness.pointer_move(a.x0 + 20.0, a.y0 + 14.0);
        settle(&mut harness);
        let row = named(harness.root_id(), "agent-run:a").unwrap();
        let show = named(row, "agent-run show in thread").unwrap();
        let show = show.get_visual_rect_no_clip();
        assert!(
            show.height() > 0.0,
            "Show in thread is not drawn under the pointer"
        );
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
                crate::state::HistoryPage {
                    has_more: true,
                    loading: false,
                },
            );
        });
        review.agents_unfolded.set(true);
        settle(&mut harness);
        let older = named(harness.root_id(), "\"Load older runs\"").unwrap();
        // Under the page's fold, past the walkthrough's card.
        older.scroll_to(None);
        settle(&mut harness);
        click(&mut harness, "\"Load older runs\"");
        assert!(app.item_runs.with(|pages| pages["pr"].loading));
        assert!(named(harness.root_id(), "\"Loading…\"").is_some());
    }

    /// A review's reviewers are under a row for the review, each with its
    /// raw output a word away, and the row folds.
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
        assert!(named(harness.root_id(), "\"Review #3\"").is_some());
        assert!(named(harness.root_id(), "\"head \u{2192} main\"").is_some());
        let group = group.get_visual_rect_no_clip();
        for run in ["claude-run", "codex-run"] {
            assert!(rect(&harness, &format!("agent-run:{run}")).y0 >= group.y1 - 0.5);
        }

        // A reviewer's line opens to its raw output.
        let line = named(harness.root_id(), "agent-run:claude-run").unwrap();
        let raw = named(line, "agent-reviewer raw output").unwrap();
        raw.scroll_to(None);
        settle(&mut harness);
        let raw = raw.get_visual_rect_no_clip();
        harness.click(raw.center().x, raw.center().y);
        settle(&mut harness);
        assert!(review
            .agents_open_runs
            .with(|open| open.contains("claude-run")));

        let group = named(harness.root_id(), "agent-review-group:group-3").unwrap();
        group.scroll_to(None);
        settle(&mut harness);
        let group = group.get_visual_rect_no_clip();
        harness.click(group.center().x, group.center().y);
        settle(&mut harness);
        assert!(
            !shown(&harness, "agent-run:claude-run"),
            "the review did not fold"
        );
        assert!(shown(&harness, "agent-run:q"));
    }

    /// A run's row and a review's say when they ran, and keep up with
    /// the clock.
    #[test]
    fn runs_and_reviews_say_when_they_ran() {
        let (mut harness, app, review) = pull_request_harness();
        let attempt = perch_github::RunRecord {
            owner: perch_github::RunOwner::Review {
                item_id: "pr".into(),
                reviewer_run_id: "attempt-claude".into(),
            },
            purpose: perch_github::comments::RunPurpose::WorktreeReview,
            ..record("claude-run", perch_github::RunStatus::Completed, 0)
        };
        add(
            app,
            &[attempt, record("q", perch_github::RunStatus::Completed, 30)],
        );
        app.review_groups.update(|all| {
            all.insert(
                "pr".into(),
                Arc::new(vec![super::super::model::tests::review_summary(
                    "group-3",
                    &["attempt-claude"],
                )]),
            );
        });
        app.now.set(at("2026-09-19T12:40:00Z"));
        review.agents_unfolded.set(true);
        settle(&mut harness);
        let when = |harness: &HeadlessHarness, row: &str, label: &str| {
            let row = named(harness.root_id(), row).unwrap();
            named(row, label).unwrap().debug_name()
        };
        assert!(when(&harness, "agent-run:q", "agent-run-when").contains("\"2h\""));
        assert!(
            when(&harness, "agent-review-group:group-3", "review-group-when").contains("\"2h\"")
        );

        app.now.set(at("2026-09-19T13:40:00Z"));
        settle(&mut harness);
        assert!(when(&harness, "agent-run:q", "agent-run-when").contains("\"3h\""));
    }

    /// A review opens to what it found, worst first, each finding its
    /// first line without the severity its comment opens with, where it
    /// is, and Show, which goes to its thread. A dismissed finding's
    /// thread, which the Open filter hides, is shown under All; one
    /// dismissed with its draft has an empty thread, so it says the title
    /// it was reported under and has no Show.
    #[test]
    fn a_review_lists_its_findings_and_show_goes_to_the_thread() {
        let (mut harness, app, review) = pull_request_harness();
        app.review_groups.update(|all| {
            all.insert(
                "pr".into(),
                Arc::new(vec![super::super::model::tests::review_summary(
                    "group-3",
                    &[],
                )]),
            );
        });
        let when = at("2026-09-19T10:00:00Z");
        let thread = |id: &str, body: &str, line: u32| perch_github::CommentThread {
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
                author: perch_github::Author::Agent {
                    backend: "claude-code".into(),
                    model: "opus".into(),
                },
                body: body.into(),
                created_at: when,
                run: None,
                github_id: None,
                node_id: None,
                html_url: None,
                diff_hunk: None,
                reactions: Default::default(),
            }],
        };
        let mut emptied = thread("t-emptied", "", 9);
        emptied.messages.clear();
        review.comment_threads.set(Arc::new(vec![
            thread("t-low", "**Low · A comment is stale.**", 3),
            thread("t-high", "**High · The lock is held across an await.**", 7),
            thread("t-dismissed", "**Note · A name could be shorter.**", 5),
            emptied,
        ]));
        let finding =
            |id: &str, thread: &str, severity| crate::review::model::ReviewFindingSummary {
                finding: perch_github::Finding {
                    id: id.into(),
                    item_id: "pr".into(),
                    stable_key: id.into(),
                    thread_id: thread.into(),
                    created_group_id: Some("group-3".into()),
                    disposition: perch_github::FindingDisposition::Open,
                    verification: perch_github::FindingVerification::Present,
                    dismissal_reason: None,
                    possible_duplicate_of: None,
                    duplicate_of: None,
                    last_verified_snapshot_id: None,
                    last_verified_head_sha: None,
                    last_checked_snapshot_id: None,
                    last_checked_head_sha: None,
                    created_at: when,
                    updated_at: when,
                },
                severity,
                attributions: Vec::new(),
                seen_in_reviews: vec![3],
                backend: None,
                model: None,
                original_review_number: Some(3),
                title: format!("{id} as reported"),
            };
        let dismissed = |id: &str, thread: &str| {
            let mut summary = finding(id, thread, perch_github::FindingSeverity::Note);
            summary.finding.disposition = perch_github::FindingDisposition::Dismissed;
            summary
        };
        app.update_review_finding(
            "pr".into(),
            finding("f-low", "t-low", perch_github::FindingSeverity::Low),
        );
        app.update_review_finding(
            "pr".into(),
            finding("f-high", "t-high", perch_github::FindingSeverity::High),
        );
        app.update_review_finding("pr".into(), dismissed("f-dismissed", "t-dismissed"));
        app.update_review_finding("pr".into(), dismissed("f-emptied", "t-emptied"));
        review.apply_review_finding_filter(
            crate::review::model::ReviewFindingFilter::Open,
            &app.review_findings_for("pr"),
        );
        review.agents_unfolded.set(true);
        settle(&mut harness);

        let rows = named(harness.root_id(), "review-group-findings")
            .expect("the findings")
            .children();
        assert_eq!(rows.len(), 4);
        let first = rows[0];
        assert!(
            named(first, "\"The lock is held across an await.\"").is_some(),
            "worst first"
        );
        assert!(named(first, "\"a.rs:7\"").is_some());
        assert!(named(rows[1], "\"A comment is stale.\"").is_some());
        let (shown, emptied) = match named(rows[2], "\"A name could be shorter.\"") {
            Some(_) => (rows[2], rows[3]),
            None => (rows[3], rows[2]),
        };
        assert!(named(shown, "\"A name could be shorter.\"").is_some());
        assert!(
            named(emptied, "\"f-emptied as reported\"").is_some(),
            "blank line"
        );
        assert!(
            named(emptied, "review-finding show").is_none(),
            "nothing to show"
        );

        let hover = |harness: &mut HeadlessHarness, row: floem::ViewId| {
            row.scroll_to(None);
            settle(harness);
            let at = row.get_visual_rect_no_clip();
            harness.pointer_move(at.x0 + 20.0, at.center().y);
            settle(harness);
        };
        assert!(
            named(first, "review-finding show")
                .is_some_and(|show| show.get_visual_rect_no_clip().height() == 0.0),
            "Show is drawn with no row under the pointer"
        );
        hover(&mut harness, first);
        let show = named(first, "review-finding show").unwrap();
        let show = show.get_visual_rect_no_clip();
        harness.click(show.center().x, show.center().y);
        settle(&mut harness);
        assert_eq!(
            review.focus_card.get(),
            Some(crate::review::doc::CardId::Thread("t-high".into()))
        );

        hover(&mut harness, shown);
        let show = named(shown, "review-finding show").unwrap();
        let show = show.get_visual_rect_no_clip();
        harness.click(show.center().x, show.center().y);
        settle(&mut harness);
        assert_eq!(
            review.review_finding_filter.get(),
            crate::review::model::ReviewFindingFilter::All
        );
        assert_eq!(
            review.pending_focus_card.get(),
            None,
            "the card never took it"
        );
        assert_eq!(
            review.focus_card.get(),
            Some(crate::review::doc::CardId::Thread("t-dismissed".into()))
        );
    }

    /// A review is listed with or without its runs loaded, even one no
    /// reviewer was admitted to, with what it was asked to focus on.
    #[test]
    fn a_review_is_listed_without_its_runs_and_says_what_it_was_asked() {
        let (mut harness, app, review) = pull_request_harness();
        let mut empty = super::super::model::tests::review_summary("group-3", &[]);
        empty.group.status = perch_github::ReviewGroupStatus::Empty;
        empty.group.instructions = Some("Review rfcs/multi_get.md only.".into());
        app.review_groups.update(|all| {
            all.insert("pr".into(), Arc::new(vec![empty]));
        });
        settle(&mut harness);
        assert!(
            shown(&harness, "agents section"),
            "a review alone brings the section"
        );
        review.agents_unfolded.set(true);
        settle(&mut harness);
        assert!(shown(&harness, "agent-review-group:group-3"));
        assert!(named(harness.root_id(), "review-group-instructions").is_some());
        assert!(named(harness.root_id(), "\"No reviewer attempts were admitted\"").is_some());
        assert!(
            named(harness.root_id(), "Review latest").is_none(),
            "no Review latest"
        );
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
