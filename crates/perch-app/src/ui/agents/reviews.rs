//! The open item's agent reviews, in the Review popover under the picker
//! and Run: a line for each, newest first, the latest shown and the older
//! behind a count. A line says who reviewed and what they found, or how
//! the review runs, with Retry or Stop at its end, and opens in place to
//! its reviewers, each with its raw output a word away, and its findings,
//! each a way to its line in Code.
//!
//! A review's output is its findings, which are threads on the page; the
//! reviewers' raw responses are the data they were read from, kept a word
//! away rather than shown first, and shown in a sheet of their own, since
//! the popover is no place for a transcript.
//!
//! On Perch's grey, as everything Perch did itself is: depth comes from
//! status colour, marks and hover washes, never a white card.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::cards::{finding_severity_label, severity_inks, tip};
use crate::review::model::{ReviewFindingSummary, ReviewGroupSummary, ReviewState};
use crate::review::{review_launch, review_menu};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{badge, clickable, icon_action, sidebar_glyph};

use super::model::{
    duration_label, elapsed_label, group_review_runs, item_runs, seconds_between,
    with_every_review, AgentReviewRunGroup, AgentRunEntry, AgentRunStatus, AgentRunSummary,
};
use super::{live_dot, status_color};

/// How tall an open review's findings get before they scroll, so a long
/// list leaves the popover room for the rest.
const FINDINGS_MAX_HEIGHT: f64 = 240.0;

/// The open item's reviews, newest first: one for each review whose
/// record has arrived, and one for a review just started whose reviewer's
/// run came before it. Tracked.
pub(crate) fn entries(app: AppState, review: ReviewState) -> Vec<AgentReviewRunGroup> {
    let Some(item_id) = review
        .item
        .with(|item| item.as_ref().map(|item| item.id.clone()))
    else {
        return Vec::new();
    };
    let groups = app.review_groups_for(&item_id);
    let runs = app
        .agent_runs
        .with(|runs| item_runs(runs, &item_id, &groups));
    let mut entries: Vec<AgentReviewRunGroup> =
        with_every_review(group_review_runs(&runs, &groups), &groups)
            .into_iter()
            .filter_map(|entry| match entry {
                AgentRunEntry::ReviewGroup(group) => Some(group),
                AgentRunEntry::Run(_) => None,
            })
            .collect();
    // One whose record has not arrived is the newest there is.
    entries.sort_by_key(|group| {
        std::cmp::Reverse(
            group
                .summary
                .as_ref()
                .map_or(u64::MAX, |summary| summary.group.number),
        )
    });
    entries
}

/// A key for a review's line that changes when what the line shows does:
/// how it stands, its reviewers' runs and how they stand, and its counts.
pub(crate) fn entry_key(group: &AgentReviewRunGroup) -> String {
    let status = group
        .summary
        .as_ref()
        .map(|summary| {
            let done = summary
                .attempts
                .iter()
                .filter(|attempt| attempt.phase.is_terminal())
                .count();
            let g = &summary.group;
            format!(
                "{}:{done}:{}/{}/{}/{}",
                g.status.name(),
                g.new_findings,
                g.still_present,
                g.verified_fixed,
                g.not_verified
            )
        })
        .unwrap_or_default();
    let runs = group
        .runs
        .iter()
        .map(|run| format!("{}={}", run.id, run.status.label()))
        .collect::<Vec<_>>()
        .join(",");
    format!("{}:{status}:{runs}", group.key)
}

/// How a review stands, as a run's would: working until every reviewer
/// is done, then done, failed or stopped as it ended.
pub(crate) fn group_status(group: &AgentReviewRunGroup) -> AgentRunStatus {
    use perch_github::ReviewGroupStatus as G;
    match group.summary.as_ref().map(|summary| summary.group.status) {
        None => group
            .runs
            .first()
            .map_or(AgentRunStatus::Running, |run| run.status),
        Some(G::Preparing | G::Reviewing | G::Reconciling) => AgentRunStatus::Running,
        Some(G::Stopping) => AgentRunStatus::Stopping,
        Some(G::Empty | G::Completed) => AgentRunStatus::Completed,
        Some(G::Failed) => AgentRunStatus::Failed,
        Some(G::Stopped) => AgentRunStatus::Stopped,
        Some(G::Interrupted) => AgentRunStatus::Interrupted,
    }
}

/// Who reviewed: `Claude, Codex`, from its attempts, or its runs before
/// its record has arrived.
pub(crate) fn reviewers_label(group: &AgentReviewRunGroup) -> String {
    let mut names: Vec<String> = match &group.summary {
        Some(summary) => summary
            .attempts
            .iter()
            .map(|attempt| review_launch::backend_label_from_name(&attempt.backend).to_string())
            .collect(),
        None => group
            .runs
            .iter()
            .map(|run| run.backend_label().to_string())
            .collect(),
    };
    names.dedup();
    names.join(", ")
}

/// What a finished review found, in the counts' own words, each kind it
/// has: `3 new · 1 still present`.
fn found_label(summary: &ReviewGroupSummary) -> String {
    let g = &summary.group;
    let found = [
        (g.new_findings, "new"),
        (g.still_present, "still present"),
        (g.verified_fixed, "verified fixed"),
        (g.not_verified, "not verified"),
    ]
    .iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, words)| format!("{count} {words}"))
    .collect::<Vec<_>>();
    if found.is_empty() {
        "nothing found".to_string()
    } else {
        found.join(" \u{b7} ")
    }
}

/// A review's line: `Review #2 · Gemini · 3 new · 1 still present`, or,
/// while it runs, how long it has; once it failed or stopped, that.
/// The dropdown's row for a review says the same after its title.
pub(crate) fn line_text(group: &AgentReviewRunGroup, now: jiff::Timestamp) -> String {
    let number = group
        .summary
        .as_ref()
        .map(|summary| format!("Review #{}", summary.group.number))
        .unwrap_or_else(|| "Review".to_string());
    let how = how_label(group, now);
    [number, reviewers_label(group), how]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

/// How a review stands, after who ran it.
pub(crate) fn how_label(group: &AgentReviewRunGroup, now: jiff::Timestamp) -> String {
    use perch_github::ReviewGroupStatus as G;
    let running = || match group.ran_at() {
        Some(at) => format!("running {}", duration_label(seconds_between(at, now))),
        None => "starting".to_string(),
    };
    match &group.summary {
        None => match group.runs.first().map(|run| run.status) {
            Some(status) if !status.is_active() => status.label().to_lowercase(),
            _ => running(),
        },
        Some(summary) => match summary.group.status {
            status if status.is_active() => running(),
            G::Failed => "failed".to_string(),
            G::Stopped => "stopped".to_string(),
            G::Interrupted => "interrupted".to_string(),
            G::Empty => "no reviewer admitted".to_string(),
            _ => found_label(summary),
        },
    }
}

/// The attempts Retry runs again: each reviewer that failed, stopped or
/// was interrupted, once the review is over.
pub(crate) fn retryable_attempts(group: &AgentReviewRunGroup) -> Vec<String> {
    use perch_github::ReviewerPhase as P;
    match &group.summary {
        Some(summary) if summary.group.status.is_active() => Vec::new(),
        Some(summary) => summary
            .attempts
            .iter()
            .filter(|attempt| matches!(attempt.phase, P::Failed | P::Stopped | P::Interrupted))
            .map(|attempt| attempt.id.clone())
            .collect(),
        None => group
            .runs
            .iter()
            .filter(|run| run.status.may_retry())
            .filter_map(|run| run.review_attempt_id.clone())
            .collect(),
    }
}

/// The review's id while it runs, for Stop.
pub(crate) fn stoppable(group: &AgentReviewRunGroup) -> Option<String> {
    group
        .summary
        .as_ref()
        .filter(|summary| summary.group.status.is_active())
        .map(|summary| summary.group.id.clone())
}

/// The item's reviews: the latest, the rest behind a count. Absent while
/// the item has had none.
pub(crate) fn history(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let all = Memo::new(move |_| entries(app, review));
    // A review asked for that is not the latest shows the older too.
    floem::reactive::Effect::new(move |_| {
        let Some(key) = review.review_expanded.get() else {
            return;
        };
        let older = all.with(|all| all.iter().skip(1).any(|group| group.key == key));
        if older && !review.review_older_shown.get_untracked() {
            review.review_older_shown.set(true);
        }
    });
    let shown = move || {
        let mut all = all.get();
        if !review.review_older_shown.get() {
            all.truncate(1);
        }
        all
    };
    let lines = dyn_stack(shown, entry_key, move |group| {
        line(app, review, backend.clone(), group)
    })
    .debug_name("agent-reviews-lines")
    .style(|s| s.flex_col().width_full());
    let older = Label::derived(move || {
        let count = all.with(|all| all.len().saturating_sub(1));
        match (review.review_older_shown.get(), count) {
            (true, _) => "Hide older reviews".to_string(),
            (false, 1) => "1 older review".to_string(),
            (false, n) => format!("{n} older reviews"),
        }
    })
    .on_event_stop(el::Click, move |_, _| {
        review.review_older_shown.update(|shown| *shown = !*shown)
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            review.review_older_shown.update(|shown| *shown = !*shown);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .debug_name("agent-reviews-older")
    .style(move |s| {
        clickable(s)
            .width_full()
            .height(28.0)
            .items_center()
            .padding_horiz(10.0)
            .font_size(theme::TEXT_BADGE)
            .color(theme::fg_muted())
            .border_top(1.0)
            .border_color(theme::border_muted())
            .selectable(false)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .hover(|s| s.background(theme::canvas_hover()).color(theme::fg()))
            .apply_if(all.with(|all| all.len() < 2), |s| s.hide())
    });
    // The lines scroll, and give the popover back what it lacks: shrunk
    // to the room under the window's bar, it takes it from here, so ten
    // reviews, one open, never run past its bottom edge.
    // At the scroll's width whatever is in it, not at its widest line's.
    Container::new(
        Stack::vertical((lines, older)).style(|s| s.flex_col().width_full().max_width_full()),
    )
    .style(|s| s.min_width_full().max_width_full())
    .scroll()
    .debug_name("agent-reviews")
    .style(move |s| {
        s.width_full()
            .min_height(0.0)
            .flex_shrink(1.0_f32)
            .scrollbar_width(0.0)
            .background(theme::perch_bg())
            .border(1.0)
            .border_color(theme::border_muted())
            .border_radius(theme::RADIUS)
            .apply_if(all.with(Vec::is_empty), |s| s.hide())
    })
}

/// Open the review's line, or shut it. Opening one folds the reader's own
/// review to its line, and the agents' settings, so one thing at a time
/// is open in the popover.
fn toggle(review: ReviewState, key: &str) {
    let open = review
        .review_expanded
        .with_untracked(|open| open.as_deref() == Some(key));
    if !open && review.review_agents_expanded.get_untracked() {
        review.review_agents_expanded.set(false);
    }
    review.review_expanded.set((!open).then(|| key.to_string()));
}

/// One review's line, and under it, while it is open, what it found and
/// who found it.
fn line(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    group: AgentReviewRunGroup,
) -> impl IntoView {
    let key = group.key.clone();
    let open = {
        let key = key.clone();
        Memo::new(move |_| {
            review
                .review_expanded
                .with(|open| open.as_deref() == Some(key.as_str()))
        })
    };
    let status = group_status(&group);
    let text_group = group.clone();
    let mark = if status.is_active() {
        crate::review::run_view::spinner(theme::running)
            .style(|s| s.size(12.0, 12.0).flex_shrink(0.0_f32))
            .into_any()
    } else {
        let color = status_color(status);
        Empty::new()
            .style(move |s| {
                s.size(7.0, 7.0)
                    .margin_horiz(2.5)
                    .border_radius(3.5)
                    .background(color())
                    .flex_shrink(0.0_f32)
            })
            .into_any()
    };
    let failed = matches!(
        status,
        AgentRunStatus::Failed | AgentRunStatus::Interrupted | AgentRunStatus::Stopped
    );
    let actions = {
        let mut actions: Vec<floem::AnyView> = Vec::new();
        let retry = retryable_attempts(&group);
        if !retry.is_empty() {
            let backend = backend.clone();
            actions.push(
                tip(
                    icon_action(SidebarIcon::Refresh, "agent-review retry", || true, {
                        move || {
                            for attempt in &retry {
                                backend.retry_review_attempt(attempt);
                            }
                        }
                    }),
                    "Retry".to_string(),
                )
                .into_any(),
            );
        }
        if let Some(group_id) = stoppable(&group) {
            let backend = backend.clone();
            actions.push(
                tip(
                    icon_action(
                        SidebarIcon::Stop,
                        "agent-review stop",
                        || true,
                        move || backend.stop_review_group(&group_id),
                    ),
                    "Stop review".to_string(),
                )
                .into_any(),
            );
        }
        Stack::horizontal_from_iter(actions)
            .style(|s| s.items_center().gap(2.0).flex_shrink(0.0_f32))
    };
    let click_key = key.clone();
    let press_key = key.clone();
    let head = Stack::horizontal((
        mark,
        Label::derived(move || line_text(&text_group, app.now.get()))
            .debug_name("agent-review-text")
            .style(move |s| {
                s.font_size(theme::TEXT_META)
                    .color(if failed { theme::closed() } else { theme::fg() })
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
                    .flex_basis(0.0)
                    .selectable(false)
            }),
        actions,
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
        .style(|s| s.flex_shrink(0.0_f32)),
    ))
    .on_event_stop(el::Click, move |_, _| toggle(review, &click_key))
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            toggle(review, &press_key);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .debug_name("agent-review-head")
    .style(move |s| {
        clickable(s)
            .items_center()
            .width_full()
            .gap(8.0)
            .min_height(34.0)
            .padding_left(10.0)
            .padding_right(8.0)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .hover(|s| s.background(theme::canvas_hover()))
            .apply_if(open.get(), |s| s.background(theme::accent_subtle()))
    });
    // Opened from elsewhere, an older review perhaps, the line comes into
    // view in the scrolling list: as it opens, or as it is built open.
    let head_id = head.view_id();
    floem::reactive::Effect::new(move |was: Option<bool>| {
        let open = open.get();
        if open && was == Some(false) {
            head_id.scroll_to(None);
        }
        open
    });
    let built_open = Rc::new(std::cell::Cell::new(open.get_untracked()));
    let head = head.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if built_open.replace(false) {
            head_id.scroll_to(None);
        }
    });
    let body = dyn_container(
        move || open.get(),
        move |open| {
            if open {
                body(app, review, backend.clone(), group.clone()).into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| s.width_full());
    Stack::vertical((head, body))
        .debug_name(format!("agent-review:{key}"))
        .style(|s| {
            s.flex_col()
                .width_full()
                .border_bottom(1.0)
                .border_color(theme::border_muted())
        })
}

/// What an open review holds: what it was asked to focus on and what of a
/// reviewer's output Perch could not read, its findings, its reviewers,
/// and Open snapshot while the snapshot it read is kept.
fn body(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    group: AgentReviewRunGroup,
) -> impl IntoView {
    let mut parts: Vec<floem::AnyView> = Vec::new();
    if let Some(summary) = &group.summary {
        if let Some(instructions) = summary.group.instructions.clone() {
            parts.push(
                Label::derived(move || instructions.clone())
                    .debug_name("agent-review-instructions")
                    .style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::fg_muted())
                            .text_wrap()
                            .width_full()
                            .padding_left(8.0)
                            .border_left(2.0)
                            .border_color(theme::border())
                    })
                    .into_any(),
            );
        }
        let notes = [
            review_launch::unread_summary(summary),
            review_launch::newer_changes(review, summary)
                .then(|| "The code has changed since this review.".to_string()),
        ];
        for note in notes.into_iter().flatten() {
            parts.push(
                Label::derived(move || note.clone())
                    .debug_name("agent-review-note")
                    .style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::notice_fg())
                            .text_wrap()
                            .width_full()
                    })
                    .into_any(),
            );
        }
        parts.push(findings(app, review, summary.clone()).into_any());
    }
    parts.push(reviewers(app, review, backend.clone(), &group).into_any());
    // A review whose own record has not arrived, as just after it starts,
    // is found by way of a reviewer's run, which names it.
    let open: Option<Rc<dyn Fn()>> = match group.summary.clone() {
        Some(summary) => {
            let openable = (summary.snapshot.manifest_path.is_some()
                && summary.snapshot.diff_path.is_some())
                || summary.group.status == perch_github::ReviewGroupStatus::Empty;
            openable.then(|| -> Rc<dyn Fn()> {
                Rc::new(move || {
                    let request_id =
                        backend.open_review_snapshot(&summary.group.item_id, &summary.group.id);
                    app.begin_historical_review(review, summary.clone(), request_id);
                })
            })
        }
        None => group
            .runs
            .first()
            .and_then(|run| Some((run.item_id.clone()?, run.review_attempt_id.clone()?)))
            .map(|(item_id, attempt_id)| -> Rc<dyn Fn()> {
                Rc::new(move || {
                    let navigation_id = backend.view_review_attempt_owner(&item_id, &attempt_id);
                    app.begin_agent_review_navigation(
                        review,
                        item_id.clone(),
                        attempt_id.clone(),
                        navigation_id,
                    );
                })
            }),
    };
    if let Some(open) = open {
        parts.push(
            super::text_action(
                || "Open snapshot".to_string(),
                false,
                move || {
                    review_menu::close(review, false);
                    open();
                },
            )
            .debug_name("agent-review open snapshot")
            .into_any(),
        );
    }
    Stack::vertical_from_iter(parts)
        .debug_name("agent-review-body")
        .style(|s| {
            s.flex_col()
                .width_full()
                .gap(10.0)
                .padding_top(4.0)
                .padding_bottom(12.0)
                .padding_horiz(10.0)
        })
}

#[derive(Debug, Clone, PartialEq)]
struct FindingRow {
    thread_id: String,
    severity: perch_github::FindingSeverity,
    line: String,
    /// The file and line it is on, to go to, and how the row names it.
    at: Option<(String, usize)>,
    place: Option<String>,
    /// Fixed or Dismissed, once it is either.
    settled: Option<&'static str>,
    /// Whether its thread has anything in it to show.
    shown: bool,
}

/// What the review found, worst first: each finding's severity, its first
/// line, and where it is, which the row goes to. Read live, so a finding
/// verified or dismissed elsewhere says so here.
fn findings(app: AppState, review: ReviewState, summary: ReviewGroupSummary) -> impl IntoView {
    let group_id = summary.group.id.clone();
    let item_id = summary.group.item_id.clone();
    let row_item_id = item_id.clone();
    let done = !summary.group.status.is_active();
    let rows = Memo::new(move |_| {
        let mut found: Vec<ReviewFindingSummary> = app
            .review_findings_for(&item_id)
            .iter()
            .filter(|f| f.finding.created_group_id.as_deref() == Some(group_id.as_str()))
            .cloned()
            .collect();
        found.sort_by_key(|f| std::cmp::Reverse(severity_rank(f.severity)));
        review.comment_threads.with(|threads| {
            found
                .into_iter()
                .map(|f| {
                    let thread = threads.iter().find(|t| t.id == f.finding.thread_id);
                    // A finding dismissed with its draft keeps a thread with
                    // nothing in it, which is not drawn: its line is the
                    // title it was reported under.
                    let first = thread.and_then(|t| t.messages.first());
                    let line = finding_line(first.map_or(f.title.as_str(), |m| m.body.as_str()));
                    let anchor = thread.and_then(|t| t.anchor.as_ref());
                    FindingRow {
                        thread_id: f.finding.thread_id.clone(),
                        severity: f.severity,
                        line,
                        at: anchor.map(|a| (a.path.clone(), a.start_line.min(a.line) as usize)),
                        place: anchor.map(|a| {
                            let file = a.path.rsplit('/').next().unwrap_or(&a.path);
                            format!("{file}:{}", a.line)
                        }),
                        settled: settled_word(&f.finding),
                        shown: first.is_some(),
                    }
                })
                .collect::<Vec<_>>()
        })
    });
    dyn_container(
        move || rows.get(),
        move |rows| {
            if rows.is_empty() {
                return Label::derived(move || {
                    if done { "Nothing found." } else { "" }.to_string()
                })
                .style(move |s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_subtle())
                        .apply_if(!done, |s| s.hide())
                })
                .into_any();
            }
            let item_id = row_item_id.clone();
            Container::new(
                Stack::vertical_from_iter(
                    rows.into_iter()
                        .enumerate()
                        .map(|(i, row)| finding_row(app, review, &item_id, row, i == 0).into_any()),
                )
                .style(|s| s.flex_col().width_full().max_width_full()),
            )
            .style(|s| s.min_width_full().max_width_full())
            .scroll()
            .debug_name("agent-review-findings")
            .style(|s| {
                s.width_full()
                    .scrollbar_width(0.0)
                    .max_height(FINDINGS_MAX_HEIGHT)
                    .border(1.0)
                    .border_color(theme::border_muted())
                    .border_radius(theme::RADIUS)
            })
            .into_any()
        },
    )
    // Nothing at all while a running review has found nothing yet.
    .style(move |s| {
        s.width_full()
            .apply_if(!done && rows.with(|r| r.is_empty()), |s| s.hide())
    })
}

/// A finding: its severity, its first line and where it is. A press goes
/// to its line in Code and shuts the popover; one with no line, as on a
/// thread about the whole change, goes to its thread on Conversation.
fn finding_row(
    app: AppState,
    review: ReviewState,
    item_id: &str,
    row: FindingRow,
    first: bool,
) -> impl IntoView {
    let (ink, fill) = severity_inks(row.severity);
    let severity = finding_severity_label(row.severity);
    let FindingRow {
        thread_id,
        line,
        at,
        place,
        settled,
        shown,
        ..
    } = row;
    let item_id = item_id.to_string();
    let faded = settled.is_some();
    let linked = at.is_some() || shown;
    let go = move || {
        if let Some((path, line)) = &at {
            review_menu::close(review, false);
            review.go_to_file(path, Some((line.saturating_sub(1), 0)));
        } else if shown {
            review_menu::close(review, false);
            super::show_thread(app, review, &item_id, &thread_id);
        }
    };
    // Where it is, and whether it is settled, under its line, which has
    // the row's width to itself.
    let meta = [place, settled.map(str::to_string)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    let has_meta = !meta.is_empty();
    Stack::horizontal((
        badge(move || severity.to_string(), ink, fill, None)
            .style(|s| s.flex_shrink(0.0_f32).margin_top(1.0)),
        Stack::vertical((
            Label::derived(move || line.clone()).style(move |s| {
                s.font_size(theme::TEXT_META)
                    .color(if faded {
                        theme::fg_subtle()
                    } else {
                        theme::fg()
                    })
                    .text_ellipsis()
                    .width_full()
                    .min_width(0.0)
                    .selectable(false)
            }),
            Label::derived(move || meta.clone())
                .debug_name("agent-review-place")
                .style(move |s| {
                    s.font_size(theme::TEXT_BADGE)
                        .font_family(theme::CODE_FONT_STACK.to_string())
                        .color(theme::fg_subtle())
                        .text_ellipsis()
                        .width_full()
                        .min_width(0.0)
                        .selectable(false)
                        .apply_if(!has_meta, |s| s.hide())
                }),
        ))
        .style(|s| {
            s.flex_col()
                .gap(2.0)
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| go())
    .debug_name("agent-review-finding")
    .style(move |s| {
        let s = s
            .items_start()
            .width_full()
            .gap(8.0)
            .min_height(30.0)
            .padding_horiz(8.0)
            .padding_vert(6.0)
            .apply_if(!first, |s| {
                s.border_top(1.0).border_color(theme::border_muted())
            });
        if linked {
            clickable(s).hover(|s| s.background(theme::canvas_hover()))
        } else {
            s
        }
    })
}

/// Who reviewed: a line for each reviewer's run, with Raw output, or,
/// before its runs have loaded, how each attempt went.
fn reviewers(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    group: &AgentReviewRunGroup,
) -> impl IntoView {
    if group.runs.is_empty() {
        let attempts = group
            .summary
            .as_ref()
            .map(review_launch::attempt_summary)
            .unwrap_or_default();
        return Label::derived(move || attempts.clone())
            .debug_name("agent-review-attempts")
            .style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .text_wrap()
                    .width_full()
            })
            .into_any();
    }
    Stack::vertical_from_iter(
        group
            .runs
            .iter()
            .map(|run| reviewer(app, review, backend.clone(), run.clone()).into_any()),
    )
    .debug_name("agent-review-reviewers")
    .style(|s| s.flex_col().width_full().gap(2.0))
    .into_any()
}

/// A reviewer: a dot for how it stands, what ran it, what it is doing or
/// how it went, and Raw output, which opens what it said in a sheet.
fn reviewer(
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
    let dot = dyn_container(
        move || run.with(|run| run.status),
        move |status| {
            if matches!(status, AgentRunStatus::Starting | AgentRunStatus::Running) {
                live_dot(app, 6.0).into_any()
            } else {
                let color = status_color(status);
                Empty::new()
                    .style(move |s| s.size(6.0, 6.0).border_radius(3.0).background(color()))
                    .into_any()
            }
        },
    )
    .style(|s| s.flex_shrink(0.0_f32));
    Stack::horizontal((
        dot,
        Label::derived(move || {
            run.with(|run| {
                crate::review::threads::agent_name(&run.backend, &run.model, &run.effort)
            })
        })
        .style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg())
                .text_ellipsis()
                .min_width(0.0)
                .flex_shrink(1.0_f32)
                .selectable(false)
        }),
        Label::derived(move || {
            let run = run.get();
            match run
                .latest_activity
                .clone()
                .filter(|_| run.status.is_active())
            {
                Some(activity) => activity,
                None => format!(
                    "{} \u{b7} {}",
                    run.status.label(),
                    elapsed_label(&run, app.now.get())
                ),
            }
        })
        .debug_name("agent-review-reviewer-status")
        .style(move |s| {
            let status = run.with(|run| run.status);
            s.font_size(theme::TEXT_BADGE)
                .color(match status {
                    AgentRunStatus::Completed => theme::fg_subtle(),
                    status if status.is_active() => theme::fg_muted(),
                    status => status_color(status)(),
                })
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .selectable(false)
        }),
        super::text_action(
            || "Raw output".to_string(),
            false,
            move || super::show_output(app, review, &backend, &id),
        )
        .debug_name("agent-review raw output"),
    ))
    .debug_name(format!(
        "agent-review-reviewer:{}",
        run.with_untracked(|run| run.id.clone())
    ))
    .style(|s| s.items_center().width_full().gap(8.0).min_height(28.0))
}

/// A finding's first line as a list shows it, as plain words: without the
/// bold its comment opens with, the severity the chip beside it says, or
/// the backticks round its code.
fn finding_line(body: &str) -> String {
    let line = crate::review::timeline::first_line(body).replace('`', "");
    let line = line.trim_matches(['*', '_']).trim();
    for word in ["Critical", "High", "Medium", "Low", "Note"] {
        if let Some(rest) = line.strip_prefix(word) {
            let rest = rest.trim_start_matches(['*', '_']).trim_start();
            if let Some(rest) = rest.strip_prefix('\u{b7}') {
                return rest.trim().trim_matches(['*', '_']).trim().to_string();
            }
        }
    }
    line.to_string()
}

/// What a finding that is done with says beside its line.
fn settled_word(finding: &perch_github::Finding) -> Option<&'static str> {
    use perch_github::{FindingDisposition, FindingVerification};
    if finding.disposition == FindingDisposition::Dismissed {
        Some("Dismissed")
    } else if finding.disposition == FindingDisposition::VerifiedFixed
        || finding.verification == FindingVerification::Fixed
    {
        Some("\u{2713} Fixed")
    } else {
        None
    }
}

fn severity_rank(severity: perch_github::FindingSeverity) -> u8 {
    use perch_github::FindingSeverity;
    match severity {
        FindingSeverity::Critical => 5,
        FindingSeverity::High => 4,
        FindingSeverity::Medium => 3,
        FindingSeverity::Low => 2,
        FindingSeverity::Note => 1,
        FindingSeverity::Unknown => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::finding_line;

    use std::sync::Arc;

    use floem::headless::HeadlessHarness;
    use floem::reactive::{SignalGet, SignalUpdate};
    use perch_github::RunStatus;

    use crate::backend::Command;
    use crate::review::model::{ItemTab, ReviewState};
    use crate::state::AppState;
    use crate::ui::agents::test_support::{
        add, at, click, named, rect, review, reviewer, sent, set_reviews, settle, shown, thread,
        window,
    };

    #[test]
    fn a_findings_line_drops_the_severity_and_bold_its_comment_opens_with() {
        assert_eq!(
            finding_line("**Medium · Parsing scheduler_options repeats the parse.**\n\nMore."),
            "Parsing scheduler_options repeats the parse."
        );
        assert_eq!(finding_line("Plain words."), "Plain words.");
        assert_eq!(
            finding_line("Why does `fence` take it?"),
            "Why does fence take it?"
        );
    }

    /// A finding of review `group`, in thread `thread`.
    fn finding(
        id: &str,
        thread: &str,
        group: &str,
        severity: perch_github::FindingSeverity,
    ) -> crate::review::model::ReviewFindingSummary {
        let when = crate::ui::agents::test_support::at("2026-09-19T10:00:00Z");
        crate::review::model::ReviewFindingSummary {
            finding: perch_github::Finding {
                id: id.into(),
                item_id: "pr".into(),
                stable_key: id.into(),
                thread_id: thread.into(),
                created_group_id: Some(group.into()),
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
        }
    }

    /// Three reviews of the pull request, #3 by the reviewer `claude-run`
    /// with two findings, and the popover open.
    fn popover() -> (HeadlessHarness, AppState, ReviewState) {
        let (mut harness, app, state, _) = window();
        add(
            app,
            &[reviewer(
                "claude-run",
                "attempt-claude",
                RunStatus::Completed,
                5,
            )],
        );
        let mut latest = review(3, &["attempt-claude"]);
        latest.group.new_findings = 2;
        set_reviews(app, vec![review(1, &[]), review(2, &[]), latest]);
        state.set_comment_threads(Arc::new(vec![
            thread("t-low", "**Low · A comment is stale.**", 3),
            thread("t-high", "**High · The lock is held across an await.**", 7),
        ]));
        for (id, thread, severity) in [
            ("f-low", "t-low", perch_github::FindingSeverity::Low),
            ("f-high", "t-high", perch_github::FindingSeverity::High),
        ] {
            app.update_review_finding("pr".into(), finding(id, thread, "group-3", severity));
        }
        settle(&mut harness);
        click(&mut harness, "review-control");
        assert!(state.review_open.get());
        (harness, app, state)
    }

    fn line_y(harness: &HeadlessHarness, key: &str) -> f64 {
        rect(harness, &format!("agent-review:{key}")).y0
    }

    /// The popover lists the latest review under Run, what it found in
    /// the counts' words, and the older ones behind a count, which shows
    /// them newest first.
    #[test]
    fn the_popover_lists_the_latest_review_and_the_older_behind_a_count() {
        let (mut harness, _app, state) = popover();
        let agents = rect(&harness, "review-agents-summary");
        assert!(shown(&harness, "agent-review:group-3"));
        assert!(
            line_y(&harness, "group-3") > agents.y1,
            "the review is over Run"
        );
        assert!(named(harness.root_id(), "Review #3 · Claude · 2 new").is_some());
        assert!(
            !shown(&harness, "agent-review:group-2"),
            "an older review shows"
        );
        assert!(named(harness.root_id(), "\"2 older reviews\"").is_some());

        click(&mut harness, "agent-reviews-older");
        assert!(state.review_older_shown.get());
        assert!(line_y(&harness, "group-3") < line_y(&harness, "group-2"));
        assert!(line_y(&harness, "group-2") < line_y(&harness, "group-1"));
        assert!(named(harness.root_id(), "\"Hide older reviews\"").is_some());
    }

    /// A review's line opens in place to its findings, worst first, and
    /// its reviewers, and folds the reader's own review to its line; Open
    /// there folds the review again.
    #[test]
    fn a_review_opens_in_place_and_folds_your_review() {
        let (mut harness, _app, state) = popover();
        assert!(
            named(harness.root_id(), "submit-review-folded").is_none(),
            "your review is folded at first"
        );
        click(&mut harness, "agent-review-head");
        assert_eq!(state.review_expanded.get().as_deref(), Some("group-3"));
        assert!(shown(&harness, "submit-review-folded"));
        assert!(shown(&harness, "agent-review-reviewer:claude-run"));
        let findings = named(harness.root_id(), "agent-review-findings").expect("no findings");
        let rows: Vec<_> = {
            let mut rows = Vec::new();
            fn all(id: floem::ViewId, rows: &mut Vec<floem::ViewId>) {
                let name = id.debug_name();
                if name.contains("agent-review-finding") && !name.contains("findings") {
                    rows.push(id);
                }
                for child in id.children() {
                    all(child, rows);
                }
            }
            all(findings, &mut rows);
            rows
        };
        assert_eq!(rows.len(), 2);
        assert!(
            named(rows[0], "\"The lock is held across an await.\"").is_some(),
            "worst first"
        );
        assert!(named(rows[0], "\"a.rs:7\"").is_some());

        click(&mut harness, "submit-review-unfold");
        assert_eq!(state.review_expanded.get(), None);
        assert!(!shown(&harness, "submit-review-folded"));
    }

    /// A finding's row goes to its line in Code and shuts the popover.
    #[test]
    fn a_finding_goes_to_its_line() {
        let (mut harness, _app, state) = popover();
        state.review_expanded.set(Some("group-3".into()));
        settle(&mut harness);
        let findings = named(harness.root_id(), "agent-review-findings").unwrap();
        let high = named(findings, "\"The lock is held across an await.\"")
            .unwrap()
            .get_visual_rect_no_clip();
        harness.click(high.center().x, high.center().y);
        settle(&mut harness);
        assert!(!state.review_open.get(), "the popover stayed");
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert_eq!(
            state.pending_jump.get(),
            Some(("src/a.rs".to_string(), 6, 0)),
            "not at the finding's line"
        );
    }

    /// A reviewer's Raw output opens what it said in a sheet over the
    /// window, asking for it, and the sheet's close puts it away with the
    /// popover still open under it.
    #[test]
    fn raw_output_opens_in_its_sheet() {
        let (mut harness, _app, state) = popover();
        state.review_expanded.set(Some("group-3".into()));
        settle(&mut harness);
        let reviewer = named(harness.root_id(), "agent-review-reviewer:claude-run").unwrap();
        let raw = named(reviewer, "agent-review raw output")
            .unwrap()
            .get_visual_rect_no_clip();
        harness.click(raw.center().x, raw.center().y);
        settle(&mut harness);
        assert_eq!(state.agent_output.get().as_deref(), Some("claude-run"));
        let sheet = rect(&harness, "agent-output-panel");
        assert!(sheet.height() > 100.0, "the sheet is not up: {sheet:?}");
        assert!(sheet.width() > rect(&harness, "review-popover").width());
        assert!(shown(&harness, "agent-run-inline"));

        click(&mut harness, "agent-output close");
        assert_eq!(state.agent_output.get(), None);
        assert!(!shown(&harness, "agent-output-panel"));
        assert!(state.review_open.get(), "the popover went with the sheet");
    }

    /// An open review and a reviewer's raw output over it go dark with the
    /// system.
    #[test]
    fn goes_dark_with_the_system() {
        let (mut harness, _app, state) = popover();
        state.review_expanded.set(Some("group-3".into()));
        state.agent_output.set(Some("claude-run".into()));
        settle(&mut harness);
        assert!(shown(&harness, "agent-output-panel"));
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(
            left.is_empty(),
            "light colours left in dark:\n{}",
            left.join("\n")
        );
    }

    /// Press `action` on the line of review `key`.
    fn press(harness: &mut HeadlessHarness, key: &str, action: &str) {
        let line = named(harness.root_id(), &format!("agent-review:{key}")).expect("no line");
        let at = named(line, action)
            .unwrap_or_else(|| panic!("no {action} on {key}"))
            .get_visual_rect_no_clip()
            .center();
        harness.click(at.x, at.y);
        settle(harness);
    }

    /// A review no reviewer was admitted to is listed all the same, says
    /// so, and opens to what it was asked to focus on and its attempts.
    #[test]
    fn a_review_without_runs_says_what_it_was_asked() {
        let (mut harness, app, state, _) = window();
        let mut empty = review(4, &[]);
        empty.group.status = perch_github::ReviewGroupStatus::Empty;
        empty.group.instructions = Some("Look at the locking.".into());
        set_reviews(app, vec![empty]);
        settle(&mut harness);
        click(&mut harness, "review-control");
        assert!(shown(&harness, "agent-review:group-4"));
        assert!(named(harness.root_id(), "Review #4 · no reviewer admitted").is_some());

        click(&mut harness, "agent-review-head");
        assert_eq!(state.review_expanded.get().as_deref(), Some("group-4"));
        let instructions =
            named(harness.root_id(), "agent-review-instructions").expect("no instructions");
        assert!(instructions.get_visual_rect_no_clip().height() > 0.0);
        assert!(named(instructions, "Look at the locking.").is_some());
        let attempts = named(harness.root_id(), "agent-review-attempts").expect("no attempts");
        assert!(named(attempts, "No reviewer attempts were admitted").is_some());
    }

    /// A review running says for how long, with a spinner, and Stop on its
    /// line stops it; one that failed says so, and Retry runs its failed
    /// reviewer again.
    #[test]
    fn a_running_review_stops_and_a_failed_one_retries() {
        let (mut harness, app, state, mut commands) = window();
        let mut running = review(3, &["attempt-claude"]);
        running.group.status = perch_github::ReviewGroupStatus::Reviewing;
        running.attempts[0].phase = perch_github::ReviewerPhase::Reviewing;
        set_reviews(app, vec![running]);
        app.now.set(at("2026-09-19T11:04:00Z"));
        settle(&mut harness);
        click(&mut harness, "review-control");
        assert!(state.review_open.get());
        assert!(
            named(harness.root_id(), "Review #3 · Claude · running 4m 00s").is_some(),
            "the line does not say how long it has run"
        );
        let line = named(harness.root_id(), "agent-review:group-3").unwrap();
        assert!(named(line, "spinner").is_some(), "no spinner");
        assert!(named(line, "agent-review retry").is_none());
        sent(&mut commands);
        press(&mut harness, "group-3", "agent-review stop");
        assert!(
            sent(&mut commands).iter().any(|c| matches!(
                c,
                Command::StopReviewGroup { group_id } if group_id == "group-3"
            )),
            "Stop did not stop the review"
        );

        let mut failed = review(3, &["attempt-claude"]);
        failed.group.status = perch_github::ReviewGroupStatus::Failed;
        failed.attempts[0].phase = perch_github::ReviewerPhase::Failed;
        set_reviews(app, vec![failed]);
        settle(&mut harness);
        assert!(named(harness.root_id(), "Review #3 · Claude · failed").is_some());
        let line = named(harness.root_id(), "agent-review:group-3").unwrap();
        assert!(named(line, "agent-review stop").is_none());
        press(&mut harness, "group-3", "agent-review retry");
        assert!(
            sent(&mut commands).iter().any(|c| matches!(
                c,
                Command::RetryReviewAttempt { attempt_id } if attempt_id == "attempt-claude"
            )),
            "Retry did not run the reviewer again"
        );
        assert!(state.review_open.get(), "an action shut the popover");
    }

    /// A reviewer's run revealed before its review's record has come, as
    /// one from an item just opened from a toast is, opens its review once
    /// the record comes: the line open, and the reader's review folded.
    #[test]
    fn a_review_revealed_before_its_record_opens_once_it_comes() {
        let (mut harness, app, state, _) = window();
        add(
            app,
            &[reviewer(
                "claude-run",
                "attempt-claude",
                RunStatus::Completed,
                5,
            )],
        );
        settle(&mut harness);
        state.pending_run_reveal.set(Some("claude-run".into()));
        settle(&mut harness);
        assert!(state.review_open.get());
        assert_eq!(
            state.review_expanded.get().as_deref(),
            Some("attempt:attempt-claude")
        );

        set_reviews(app, vec![review(2, &[]), review(3, &["attempt-claude"])]);
        settle(&mut harness);
        assert_eq!(state.review_expanded.get().as_deref(), Some("group-3"));
        assert!(shown(&harness, "agent-review-reviewer:claude-run"));
        assert!(shown(&harness, "submit-review-folded"));
    }

    /// Fourteen reviews, an older one open, still fit the popover under the
    /// bar: the lines scroll, and the reader's folded review stays in it.
    #[test]
    fn many_reviews_scroll_inside_the_popover() {
        let (mut harness, app, state, _) = window();
        add(
            app,
            &[reviewer(
                "claude-run",
                "attempt-claude",
                RunStatus::Completed,
                5,
            )],
        );
        let mut reviews: Vec<_> = (1..=14).map(|number| review(number, &[])).collect();
        reviews[1] = review(2, &["attempt-claude"]);
        reviews[1].group.instructions = Some("Look at the locking. ".repeat(20));
        set_reviews(app, reviews);
        settle(&mut harness);
        click(&mut harness, "review-control");
        state.review_expanded.set(Some("group-2".into()));
        settle(&mut harness);
        assert!(state.review_older_shown.get());

        let window = harness.root_id().get_visual_rect_no_clip();
        let panel = rect(&harness, "review-popover");
        assert!(
            panel.y1 <= window.y1,
            "the popover at {panel:?} runs past the window {window:?}"
        );
        let folded = rect(&harness, "submit-review-folded");
        assert!(
            folded.height() > 0.0 && folded.y1 <= panel.y1,
            "your review at {folded:?} is past the popover {panel:?}"
        );
        let list = rect(&harness, "agent-reviews");
        assert!(list.y1 <= folded.y0, "the reviews at {list:?} run over it");
        let lines = named(harness.root_id(), "agent-reviews")
            .unwrap()
            .children()[0];
        let content = harness.get_layout_rect(lines).height();
        assert!(
            list.height() < content,
            "the reviews at {list:?} do not scroll {content} of lines"
        );
    }
}
