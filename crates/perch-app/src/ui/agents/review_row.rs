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
use crate::review::model::{ReviewFindingSummary, ReviewGroupSummary, ReviewState};
use crate::review::review_launch;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{badge, clickable, sidebar_glyph};

use super::model::AgentReviewRunGroup;
use super::section::run_row;
use super::text_action;

/// How far a review's findings and reviewers are set in under its title:
/// past the chevron, to where the title starts.
const REVIEW_INSET: f64 = 17.0;

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
    let status_backend = backend.clone();
    let early = group.clone();

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
                    theme::fg_subtle,
                )
                .style(|s| s.size(10.0, 10.0))
            },
        )
        .style(|s| s.width(REVIEW_INSET - 7.0).flex_shrink(0.0_f32)),
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
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .selectable(false)
        }),
        standing(app, &group),
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
            .items_center()
            .width_full()
            .gap(7.0)
            .min_height(24.0)
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
            let summary = group.summary.clone();
            Stack::vertical((
                match &summary {
                    Some(summary) => findings(app, review, summary.clone()).into_any(),
                    None => Empty::new().into_any(),
                },
                reviewers(app, review, backend.clone(), &group),
            ))
            .style(|s| s.flex_col().width_full().gap(10.0).margin_top(8.0))
            .into_any()
        },
    )
    .style(|s| s.width_full().padding_left(REVIEW_INSET));

    Stack::vertical((
        header,
        match &summary {
            Some(summary) => status_line(app, review, status_backend, summary.clone()).into_any(),
            None => early_snapshot(app, review, status_backend, &early).into_any(),
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

/// What the review read: `of d1dc594 against main at 05e8de2`.
fn subject(summary: &ReviewGroupSummary) -> String {
    let base: String = summary.snapshot.base_tip_sha.chars().take(7).collect();
    format!(
        "of {} against {} at {base}",
        summary.snapshot.head_label(),
        summary.snapshot.requested_base_ref
    )
}

/// How the review stands at the end of its title: how far its reviewers
/// have got while it runs, and when it ran once it is done.
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
        format!("Reviewing \u{b7} {done} of {} complete", summary.attempts.len())
    });
    Label::derived(move || match (active, &progress, ran_at) {
        (true, Some(progress), _) => progress.clone(),
        (true, None, _) => "Starting".to_string(),
        (false, _, Some(at)) => timefmt::relative(at, app.now.get()),
        (false, _, None) => String::new(),
    })
    .debug_name("review-group-when")
    .style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .color(if active {
                theme::draft_yellow()
            } else {
                theme::fg_subtle()
            })
            .flex_shrink(0.0_f32)
            .selectable(false)
    })
}

/// Under the title: what the review found, in chips, whether the code has
/// moved since, and what can be done with it; then what it was asked to
/// focus on, and what of a reviewer's output Perch could not read.
fn status_line(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    summary: ReviewGroupSummary,
) -> impl IntoView {
    let instructions = summary.group.instructions.clone();
    let unread = review_launch::unread_summary(&summary);
    let newer = summary.clone();
    Stack::vertical((
        Stack::horizontal((
            review_launch::count_chips(&summary),
            Label::derived(|| "\u{b7} code changed since".to_string())
                .debug_name("review-group-newer")
                .style(move |s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::notice_fg())
                        .selectable(false)
                        .apply_if(!review_launch::newer_changes(review, &newer), |s| s.hide())
                }),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            group_actions(app, review, backend, summary),
        ))
        .style(|s| s.items_center().width_full().gap(6.0).min_height(24.0)),
        match instructions {
            Some(instructions) => Label::derived(move || instructions.clone())
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
            None => Empty::new().into_any(),
        },
        match unread {
            Some(unread) => Label::derived(move || unread.clone())
                .debug_name("review-group-unread")
                .style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::notice_fg())
                        .text_wrap()
                        .width_full()
                })
                .into_any(),
            None => Empty::new().into_any(),
        },
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .gap(6.0)
            .margin_top(6.0)
            .padding_left(REVIEW_INSET)
    })
}

/// Open snapshot, while the snapshot the review read is kept, and Stop
/// all while it runs.
fn group_actions(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    summary: ReviewGroupSummary,
) -> impl IntoView {
    let openable = (summary.snapshot.manifest_path.is_some()
        && summary.snapshot.diff_path.is_some())
        || summary.group.status == perch_github::ReviewGroupStatus::Empty;
    let open = match openable {
        true => {
            let backend = backend.clone();
            let summary = summary.clone();
            text_action(
                || "Open snapshot".to_string(),
                false,
                move || {
                    let request_id = backend
                        .open_review_snapshot(&summary.group.item_id, &summary.group.id);
                    app.begin_historical_review(review, summary.clone(), request_id);
                },
            )
            .debug_name("review-group open snapshot")
            .into_any()
        }
        _ => Empty::new().into_any(),
    };
    let stop = match summary.group.status.is_active() {
        true => {
            let group_id = summary.group.id.clone();
            text_action(
                || "Stop all".to_string(),
                false,
                move || backend.stop_review_group(&group_id),
            )
            .debug_name("review-group stop")
            .into_any()
        }
        _ => Empty::new().into_any(),
    };
    Stack::horizontal((open, stop)).style(|s| s.items_center().gap(2.0).flex_shrink(0.0_f32))
}

/// Open snapshot on a review whose own record has not arrived, as just
/// after it starts: found by way of a reviewer's run, which names it.
fn early_snapshot(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    group: &AgentReviewRunGroup,
) -> impl IntoView {
    let Some((item_id, attempt_id)) = group
        .runs
        .first()
        .and_then(|run| Some((run.item_id.clone()?, run.review_attempt_id.clone()?)))
    else {
        return Empty::new().into_any();
    };
    Stack::horizontal((
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
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
        .debug_name("review-group open snapshot"),
    ))
    .style(|s| s.width_full().items_center().margin_top(6.0))
    .into_any()
}

/// What the review found, worst first: each finding's severity, its first
/// line, where it is, and Show, which goes to its thread. Read live, so a
/// finding verified or dismissed elsewhere says so here.
fn findings(app: AppState, review: ReviewState, summary: ReviewGroupSummary) -> impl IntoView {
    let group_id = summary.group.id.clone();
    let item_id = summary.group.item_id.clone();
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
                    let line = thread
                        .and_then(|t| t.messages.first())
                        .map(|m| finding_line(&m.body))
                        .unwrap_or_default();
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
            dyn_stack(
                move || rows.clone().into_iter().enumerate(),
                |(i, row)| format!("{i}:{}", row.thread_id),
                move |(i, row)| finding_row(review, row, i == 0, i == last),
            )
            .debug_name("review-group-findings")
            .style(|s| {
                s.flex_col()
                    .width_full()
                    .background(theme::canvas())
                    .border(1.0)
                    .border_color(theme::border_muted())
                    .border_radius(theme::RADIUS)
            })
            .into_any()
        },
    )
    .style(|s| s.width_full())
}

#[derive(Debug, Clone, PartialEq)]
struct FindingRow {
    thread_id: String,
    severity: perch_github::FindingSeverity,
    line: String,
    place: Option<String>,
    /// Fixed or Dismissed, once it is either.
    settled: Option<&'static str>,
}

fn finding_row(review: ReviewState, row: FindingRow, first: bool, _last: bool) -> impl IntoView {
    let (ink, fill) = severity_inks(row.severity);
    let severity = finding_severity_label(row.severity);
    let FindingRow {
        thread_id,
        line,
        place,
        settled,
        ..
    } = row;
    let faded = settled.is_some();
    Stack::horizontal((
        badge(move || severity.to_string(), ink, fill, None),
        Label::derived(move || line.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if faded { theme::fg_subtle() } else { theme::fg() })
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
        text_action(
            || "Show".to_string(),
            true,
            move || review.focus_card.set(Some(CardId::Thread(thread_id.clone()))),
        )
        .debug_name("review-finding show"),
    ))
    .debug_name("review-finding")
    .style(move |s| {
        s.items_center()
            .width_full()
            .gap(8.0)
            .min_height(32.0)
            .padding_left(10.0)
            .padding_right(4.0)
            .apply_if(!first, |s| s.border_top(1.0).border_color(theme::border_muted()))
    })
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
        assert_eq!(finding_line("Why does `fence` take it?"), "Why does fence take it?");
    }
}
