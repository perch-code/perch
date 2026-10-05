//! A review's row in an item's Agents section: what it reviewed and how
//! it stands, what it found, each finding a way to its thread, and who
//! reviewed, each with what it said behind Raw output. It folds.
//!
//! A review's output is its findings, which are threads on the page; the
//! reviewers' raw responses are the data they were read from, kept a
//! word away rather than shown first.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, dyn_stack, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::cards::{finding_severity_label, severity_inks};
use crate::review::doc::CardId;
use crate::review::model::{
    ReviewFindingFilter, ReviewFindingSummary, ReviewGroupSummary, ReviewState,
};
use crate::review::review_launch;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{badge, clickable, icon_action};

use super::model::{duration_label, seconds_between, AgentReviewRunGroup, AgentRunStatus};
use super::section::run_row;
use super::{status_mark, STATUS_MARK};

/// How far a review's findings and reviewers are set in under its title:
/// past its mark, to where the title starts.
const REVIEW_INSET: f64 = STATUS_MARK + 10.0;

pub(super) fn review_row(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    group: AgentReviewRunGroup,
) -> impl IntoView {
    let key = group.key.clone();
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
    let summary = group.summary.clone();

    let title = Stack::horizontal((
        status_mark(app, group_status(&group)),
        Label::derived({
            let summary = summary.clone();
            move || match &summary {
                Some(summary) => format!("Review #{}", summary.group.number),
                None => "Review".to_string(),
            }
        })
        .style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .flex_shrink(0.0_f32)
                .selectable(false)
        }),
        Label::derived({
            let summary = summary.clone();
            move || summary.as_ref().map(subject).unwrap_or_default()
        })
        .style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(theme::fg_muted())
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .selectable(false)
        }),
        standing(app, &group),
        group_actions(app, review, backend.clone(), &group),
    ))
    .style(|s| s.items_center().width_full().gap(10.0).min_height(28.0));

    let header = Stack::vertical((title, progress(&group)))
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
                .flex_col()
                .width_full()
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
        })
        .debug_name(format!("agent-review-group:{key}"));

    let body = dyn_container(
        move || collapsed.get(),
        move |collapsed| {
            if collapsed {
                return Empty::new().into_any();
            }
            let mut parts = Vec::new();
            if let Some(summary) = &group.summary {
                parts.push(findings(app, review, summary.clone()).into_any());
            }
            parts.push(reviewers(app, review, backend.clone(), &group).into_any());
            Stack::vertical_from_iter(parts)
                .style(|s| s.flex_col().width_full().gap(10.0).margin_top(10.0))
                .into_any()
        },
    )
    .style(|s| s.width_full().padding_left(REVIEW_INSET));

    Stack::vertical((
        header,
        match &summary {
            Some(summary) => status_line(review, summary.clone()).into_any(),
            None => Empty::new().into_any(),
        },
        body,
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .padding_vert(12.0)
            .padding_horiz(theme::CARD_PADDING)
            .border_top(1.0)
            .border_color(theme::border_muted())
    })
}

/// How the review stands, as a run's mark says it: working until every
/// reviewer is done, then done, failed or stopped as it ended.
fn group_status(group: &AgentReviewRunGroup) -> AgentRunStatus {
    use perch_github::ReviewGroupStatus as G;
    match group.summary.as_ref().map(|summary| summary.group.status) {
        None | Some(G::Preparing | G::Reviewing | G::Reconciling) => AgentRunStatus::Running,
        Some(G::Stopping) => AgentRunStatus::Stopping,
        Some(G::Empty | G::Completed) => AgentRunStatus::Completed,
        Some(G::Failed) => AgentRunStatus::Failed,
        Some(G::Stopped) => AgentRunStatus::Stopped,
        Some(G::Interrupted) => AgentRunStatus::Interrupted,
    }
}

/// What the review read: `d1dc594 → main`.
fn subject(summary: &ReviewGroupSummary) -> String {
    format!(
        "{} \u{2192} {}",
        summary.snapshot.head_label(),
        summary.snapshot.requested_base_ref
    )
}

/// How the review stands at the end of its title: how many reviewers are
/// done and for how long it has run while it runs, and when it ran once
/// it is done.
fn standing(app: AppState, group: &AgentReviewRunGroup) -> impl IntoView {
    let ran_at = group.ran_at();
    let active = group
        .summary
        .as_ref()
        .is_none_or(|summary| summary.group.status.is_active());
    let progress = group.summary.as_ref().map(|summary| {
        let done = summary
            .attempts
            .iter()
            .filter(|attempt| attempt.phase.is_terminal())
            .count();
        format!("{done} of {}", summary.attempts.len())
    });
    Label::derived(move || {
        let now = app.now.get();
        let elapsed = ran_at.map(|at| duration_label(seconds_between(at, now)));
        match (active, &progress, elapsed) {
            (true, Some(progress), Some(elapsed)) => format!("{progress} \u{b7} {elapsed}"),
            (true, Some(progress), None) => progress.clone(),
            (true, None, _) => "Starting".to_string(),
            (false, _, _) => ran_at.map(|at| timefmt::short(at, now)).unwrap_or_default(),
        }
    })
    .debug_name("review-group-when")
    .style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .color(if active {
                theme::running()
            } else {
                theme::fg_subtle()
            })
            .flex_shrink(0.0_f32)
            .selectable(false)
    })
}

/// While the review runs, a bar under its title with a piece for each
/// reviewer: filled once it is done, faint while it works, empty while
/// it waits.
fn progress(group: &AgentReviewRunGroup) -> impl IntoView {
    let phases: Vec<perch_github::ReviewerPhase> = group
        .summary
        .as_ref()
        .filter(|summary| summary.group.status.is_active())
        .map(|summary| {
            summary
                .attempts
                .iter()
                .map(|attempt| attempt.phase)
                .collect()
        })
        .unwrap_or_default();
    let shown = !phases.is_empty();
    Stack::horizontal_from_iter(phases.into_iter().map(|phase| {
        Empty::new().style(move |s| {
            let color = if phase.is_terminal() {
                theme::running()
            } else if phase == perch_github::ReviewerPhase::Queued {
                theme::border()
            } else {
                theme::running().with_alpha(0.3)
            };
            s.height(3.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .border_radius(1.5)
                .background(color)
        })
    }))
    .debug_name("review-group-progress")
    .style(move |s| {
        s.width_full()
            .gap(3.0)
            .margin_top(8.0)
            .padding_left(REVIEW_INSET)
            .apply_if(!shown, |s| s.hide())
    })
}

/// Under the title: what the review found, in chips, and whether the code
/// has moved since, when there is either; then what it was asked to focus
/// on, and what of a reviewer's output Perch could not read.
fn status_line(review: ReviewState, summary: ReviewGroupSummary) -> impl IntoView {
    let instructions = summary.group.instructions.clone();
    let unread = review_launch::unread_summary(&summary);
    let g = &summary.group;
    let chips = g.new_findings + g.still_present + g.verified_fixed + g.not_verified > 0;
    let newer = summary.clone();
    let changed = move || review_launch::newer_changes(review, &newer);
    let shown_changed = changed.clone();
    let mut lines = vec![Stack::horizontal((
        review_launch::count_chips(&summary).style(move |s| s.apply_if(!chips, |s| s.hide())),
        Label::derived(move || {
            if chips {
                "\u{b7} code changed since"
            } else {
                "Code changed since"
            }
            .to_string()
        })
        .debug_name("review-group-newer")
        .style(move |s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::notice_fg())
                .selectable(false)
                .apply_if(!changed(), |s| s.hide())
        }),
    ))
    .style(move |s| {
        s.items_center()
            .width_full()
            .gap(6.0)
            .min_height(24.0)
            .apply_if(!chips && !shown_changed(), |s| s.hide())
    })
    .into_any()];
    if let Some(instructions) = instructions {
        lines.push(
            Label::derived(move || instructions.clone())
                .debug_name("review-group-instructions")
                .style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_muted())
                        .line_height(theme::PROSE_LINE_HEIGHT)
                        .text_wrap()
                        .width_full()
                        .padding_left(8.0)
                        .border_left(2.0)
                        .border_color(theme::border())
                })
                .into_any(),
        );
    }
    if let Some(unread) = unread {
        lines.push(
            Label::derived(move || unread.clone())
                .debug_name("review-group-unread")
                .style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::notice_fg())
                        .text_wrap()
                        .width_full()
                })
                .into_any(),
        );
    }
    Stack::vertical_from_iter(lines).style(|s| {
        s.flex_col()
            .width_full()
            .gap(6.0)
            .margin_top(6.0)
            .padding_left(REVIEW_INSET)
    })
}

/// At the end of the title, glyphs: Stop, while the review runs, and Open
/// snapshot, while the snapshot it read is kept. A review whose own record
/// has not arrived, as just after it starts, is found by way of a
/// reviewer's run, which names it.
fn group_actions(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    group: &AgentReviewRunGroup,
) -> impl IntoView {
    let open: Option<Rc<dyn Fn()>> = match &group.summary {
        Some(summary) => {
            let openable = (summary.snapshot.manifest_path.is_some()
                && summary.snapshot.diff_path.is_some())
                || summary.group.status == perch_github::ReviewGroupStatus::Empty;
            let backend = backend.clone();
            let summary = summary.clone();
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
                let backend = backend.clone();
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
    let stop = group
        .summary
        .as_ref()
        .filter(|summary| summary.group.status.is_active())
        .map(|summary| {
            let group_id = summary.group.id.clone();
            crate::review::cards::tip(
                icon_action(
                    SidebarIcon::Stop,
                    "review-group stop",
                    || true,
                    move || backend.stop_review_group(&group_id),
                ),
                "Stop review".to_string(),
            )
            .into_any()
        })
        .unwrap_or_else(|| Empty::new().into_any());
    let open = match open {
        Some(open) => crate::review::cards::tip(
            icon_action(
                SidebarIcon::ArrowUpRight,
                "review-group open snapshot",
                || true,
                move || open(),
            ),
            "Open snapshot".to_string(),
        )
        .into_any(),
        None => Empty::new().into_any(),
    };
    Stack::horizontal((stop, open)).style(|s| s.items_center().gap(2.0).flex_shrink(0.0_f32))
}

/// What the review found, worst first: each finding's severity, its first
/// line, where it is, and Show, which goes to its thread. Read live, so a
/// finding verified or dismissed elsewhere says so here.
fn findings(app: AppState, review: ReviewState, summary: ReviewGroupSummary) -> impl IntoView {
    let group_id = summary.group.id.clone();
    let item_id = summary.group.item_id.clone();
    let show_item_id = item_id.clone();
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
                    // title it was reported under, and there is no Show.
                    let first = thread.and_then(|t| t.messages.first());
                    let line = finding_line(first.map_or(f.title.as_str(), |m| m.body.as_str()));
                    let place = thread.and_then(|t| t.anchor.as_ref()).map(|a| {
                        let file = a.path.rsplit('/').next().unwrap_or(&a.path);
                        format!("{file}:{}", a.line)
                    });
                    FindingRow {
                        thread_id: f.finding.thread_id.clone(),
                        severity: f.severity,
                        line,
                        place,
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
            let last = rows.len() - 1;
            let item_id = show_item_id.clone();
            dyn_stack(
                move || rows.clone().into_iter().enumerate(),
                |(i, row)| format!("{i}:{}", row.thread_id),
                move |(i, row)| finding_row(app, review, &item_id, row, i == 0, i == last),
            )
            .debug_name("review-group-findings")
            .style(|s| {
                s.flex_col()
                    .width_full()
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

#[derive(Debug, Clone, PartialEq)]
struct FindingRow {
    thread_id: String,
    severity: perch_github::FindingSeverity,
    line: String,
    place: Option<String>,
    /// Fixed or Dismissed, once it is either.
    settled: Option<&'static str>,
    /// Whether its thread has anything in it to show.
    shown: bool,
}

fn finding_row(
    app: AppState,
    review: ReviewState,
    item_id: &str,
    row: FindingRow,
    first: bool,
    _last: bool,
) -> impl IntoView {
    let (ink, fill) = severity_inks(row.severity);
    let severity = finding_severity_label(row.severity);
    let FindingRow {
        thread_id,
        line,
        place,
        settled,
        shown,
        ..
    } = row;
    let item_id = item_id.to_string();
    let faded = settled.is_some();
    let hovered = RwSignal::new(false);
    let row = Stack::horizontal((
        badge(move || severity.to_string(), ink, fill, None),
        Label::derived(move || line.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if faded {
                    theme::fg_subtle()
                } else {
                    theme::fg()
                })
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .selectable(false)
        }),
        match settled {
            Some(word) => Label::derived(move || word.to_string())
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::fg_subtle())
                        .flex_shrink(0.0_f32)
                })
                .into_any(),
            None => Empty::new().into_any(),
        },
        match place {
            Some(place) => Label::derived(move || place.clone())
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .font_family(theme::CODE_FONT_STACK.to_string())
                        .color(theme::fg_subtle())
                        .flex_shrink(0.0_f32)
                        .selectable(false)
                })
                .into_any(),
            None => Empty::new().into_any(),
        },
        match shown {
            true => crate::review::cards::hover_action(
                hovered,
                SidebarIcon::Thread,
                "Show in thread",
                "review-finding show",
                Rc::new(move || show_thread(app, review, &item_id, &thread_id)),
            )
            .into_any(),
            false => Empty::new().into_any(),
        },
    ))
    .debug_name("review-finding")
    .style(move |s| {
        s.items_center()
            .width_full()
            .gap(8.0)
            .min_height(32.0)
            .padding_left(10.0)
            .padding_right(4.0)
            .apply_if(!first, |s| {
                s.border_top(1.0).border_color(theme::border_muted())
            })
    });
    crate::review::cards::track_hover(row, hovered)
}

/// Bring a finding's thread into view on Conversation. One the Open
/// filter hides, as it does a dismissed or fixed finding's, turns the
/// filter to All, and is focused once its card is there.
fn show_thread(app: AppState, review: ReviewState, item_id: &str, thread_id: &str) {
    let card = CardId::Thread(thread_id.to_string());
    if review
        .hidden_review_threads
        .with_untracked(|hidden| hidden.contains(thread_id))
    {
        review.apply_review_finding_filter(
            ReviewFindingFilter::All,
            &app.review_findings_for(item_id),
        );
        review.pending_focus_card.set(Some(card));
    } else {
        review.focus_card.set(Some(card));
    }
}

/// Who reviewed: a line for each reviewer's run, or, before its runs have
/// loaded, how each attempt went.
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
            .style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .text_wrap()
                    .width_full()
            })
            .into_any();
    }
    let runs = group.runs.clone();
    dyn_stack(
        move || runs.clone(),
        |run| run.id.clone(),
        move |run| run_row(app, review, backend.clone(), run, true),
    )
    .debug_name("review-group-reviewers")
    .style(|s| s.flex_col().width_full().gap(4.0))
    .into_any()
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
}
