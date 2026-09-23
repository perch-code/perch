//! The Agents button in the bar, and the dropdown it opens: every item's
//! runs, the ones working grouped by item and then the ones that
//! finished last. A run goes to its item's Conversation, with the Agents
//! row unfolded and the run open in it.
//!
//! The button sits before the item's one action, where Submit review and
//! the review launcher open theirs, and on the pane with nothing open, so
//! the runs can be reached from anywhere. Its count is every item's
//! running and queued work, and a dot says a run is waiting on the reader.

use std::cell::RefCell;
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, ScrollExt, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{ItemTab, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, sidebar_glyph};

use super::list::queue_position;
use super::model::{
    active_groups_with_reviews, elapsed_label, history_runs_with_reviews, AgentRunStatus,
    AgentRunSummary, BackendFilter, OutcomeFilter, WorktreeFilter,
};
use super::{request_agent_runs, status_color};

/// How many finished runs the dropdown lists.
pub const RECENT_RUNS: usize = 15;

/// The button, and the dropdown under it while it is open.
pub fn button(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let count = move || {
        let (running, queued) = app.active_agent_counts();
        running + queued
    };
    let waiting = move || review.approvals.with(|pending| !pending.is_empty());
    let inner = Stack::horizontal((
        sidebar_glyph(SidebarIcon::Agents, theme::FG_MUTED)
            .style(|s| s.size(13.0, 13.0).margin_right(6.0).pointer_events_none()),
        Label::derived(|| "Agents".to_string()).style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG)
                .selectable(false)
                .pointer_events_none()
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }),
        Label::derived(move || count().to_string())
            .debug_name("agents count")
            .style(move |s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG_MUTED)
                    .height(18.0)
                    .padding_horiz(6.0)
                    .margin_left(6.0)
                    .border_radius(9.0)
                    .background(theme::BADGE_BG)
                    .items_center()
                    .justify_center()
                    .selectable(false)
                    .pointer_events_none()
                    .apply_if(count() == 0, |s| s.hide())
            }),
        Empty::new().debug_name("agents waiting").style(move |s| {
            s.size(6.0, 6.0)
                .border_radius(3.0)
                .background(theme::ACCENT)
                .margin_left(6.0)
                .pointer_events_none()
                .apply_if(!waiting(), |s| s.hide())
        }),
    ))
    .style(|s| s.items_center().pointer_events_none());
    let button = Container::new(inner);
    let button_id = button.view_id();
    let click_backend = backend.clone();
    let key_backend = backend.clone();
    let button = button
        .on_event_stop(el::Click, move |_, _| {
            toggle(app, review, &click_backend, Some(button_id))
        })
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Enter | Code::Space) {
                toggle(app, review, &key_backend, Some(button_id));
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(move |s| {
            let base = clickable(s)
                .height(30.0)
                .padding_horiz(10.0)
                .items_center()
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(9.0)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
                .hover(|s| s.background(theme::CANVAS_HOVER));
            if review.agents_dropdown_open.get() {
                base.background(theme::BADGE_BG)
            } else {
                base.background(theme::CANVAS)
            }
        })
        .debug_name("agents-button");

    Stack::new((
        button,
        dyn_container(
            move || review.agents_dropdown_open.get(),
            move |open| {
                if open {
                    popover(app, review, backend.clone()).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        .style(|s| {
            s.position(Position::Absolute)
                .inset_top(34.0)
                .inset_right(0.0)
                .z_index(100)
        }),
    ))
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Escape
            && key.modifiers.is_empty()
            && review.agents_dropdown_open.get_untracked()
        {
            close(review, true);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    // The bar it sits in lets the pointer through everywhere but its
    // controls; this is one.
    .style(|s| {
        s.position(Position::Relative)
            .flex_shrink(0.0_f32)
            .pointer_events_auto()
    })
}

/// Open the dropdown, or shut it when it is open. `opener` gets the
/// keyboard back when it shuts.
pub fn toggle(
    app: AppState,
    review: ReviewState,
    backend: &Backend,
    opener: Option<floem::ViewId>,
) {
    if review.agents_dropdown_open.get_untracked() {
        close(review, true);
    } else {
        open(app, review, backend, opener);
    }
}

/// Open the dropdown over whatever else hangs from the bar, and ask for
/// the runs it lists.
pub fn open(app: AppState, review: ReviewState, backend: &Backend, opener: Option<floem::ViewId>) {
    crate::review::submit_review::close(review, false);
    crate::review::review_launch::close_launcher(review, false);
    review.agents_dropdown_opener.set(opener);
    review.agents_dropdown_open.set(true);
    request_agent_runs(app, backend, RECENT_RUNS);
}

/// Shut the dropdown, giving the keyboard back to what had it when it
/// opened when `restore_focus`.
pub fn close(review: ReviewState, restore_focus: bool) {
    if !review.agents_dropdown_open.get_untracked() {
        return;
    }
    review.agents_dropdown_open.set(false);
    let opener = review.agents_dropdown_opener.get_untracked();
    review.agents_dropdown_opener.set(None);
    if restore_focus {
        if let Some(opener) = opener {
            opener.request_focus();
        }
    }
}

/// Go to a run: its item, on Conversation, with the Agents row unfolded
/// and the run open at the top of the page. The item open already has
/// it brought into view in place; another is opened first.
pub fn go_to_run(app: AppState, review: ReviewState, backend: &Backend, run: &AgentRunSummary) {
    close(review, false);
    let Some(item_id) = run
        .item_id
        .clone()
        .filter(|item_id| run.source_available && app.item(item_id).is_some())
    else {
        app.notice.set(Some(
            "The source for this run is no longer available".into(),
        ));
        return;
    };
    let same_item = app
        .open_item
        .with_untracked(|open| open.as_deref() == Some(item_id.as_str()))
        && review.is_showing();
    // A retained snapshot of the item is not where its runs are listed:
    // the item is opened again, as it is.
    if same_item && review.historical_review.with_untracked(Option::is_none) {
        floem::reactive::Effect::batch(|| {
            review.show_tab(ItemTab::Conversation);
            review.pending_run_reveal.set(Some(run.id.clone()));
        });
        return;
    }
    let navigation_id = backend.view_agent_owner(&item_id);
    app.begin_agent_run_navigation(item_id, run.id.clone(), navigation_id);
    app.begin_review(review, "Reading the discussion");
}

/// What the dropdown lists: every item's working runs by item, the item
/// named by its title, then the runs that finished last.
#[derive(Clone, PartialEq)]
struct Listing {
    active: Vec<(String, Vec<AgentRunSummary>)>,
    recent: Vec<AgentRunSummary>,
}

fn listing(app: AppState) -> Listing {
    let review_groups = app.review_groups.with(|all| {
        all.values()
            .flat_map(|groups| groups.iter().cloned())
            .collect::<Vec<_>>()
    });
    app.agent_runs.with(|runs| {
        let active = active_groups_with_reviews(
            runs,
            None,
            WorktreeFilter::All,
            BackendFilter::All,
            &review_groups,
        )
        .into_iter()
        .map(|group| (item_title(app, &group.runs[0]), group.runs))
        .collect();
        let mut recent = history_runs_with_reviews(
            runs,
            None,
            WorktreeFilter::All,
            BackendFilter::All,
            OutcomeFilter::All,
            &review_groups,
        );
        recent.truncate(RECENT_RUNS);
        Listing { active, recent }
    })
}

/// The title of the item a run is on, or where it ran when the item is
/// gone.
fn item_title(app: AppState, run: &AgentRunSummary) -> String {
    run.item_id
        .as_deref()
        .and_then(|id| app.item(id))
        .map(|item| item.title().to_string())
        .unwrap_or_else(|| run.group_label())
}

fn popover(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // The rows in the order they are drawn, for the arrow keys to move
    // through, and which one has the keyboard.
    let rows: Rc<RefCell<Vec<floem::ViewId>>> = Rc::new(RefCell::new(Vec::new()));
    let at = RwSignal::new(None::<usize>);
    let listing = Memo::new(move |_| listing(app));
    let build_rows = rows.clone();
    let body = dyn_container(
        move || listing.get(),
        move |listing| {
            build_rows.borrow_mut().clear();
            if listing.active.is_empty() && listing.recent.is_empty() {
                return Label::derived(|| "No agent runs yet".to_string())
                    .style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::FG_SUBTLE)
                            .padding(16.0)
                            .selectable(false)
                    })
                    .into_any();
            }
            let mut sections: Vec<floem::AnyView> = Vec::new();
            if !listing.active.is_empty() {
                sections.push(heading("Active").into_any());
                for (title, runs) in listing.active {
                    sections.push(item_heading(title).into_any());
                    for run in runs {
                        sections.push(
                            row(app, review, backend.clone(), &build_rows, at, run, None)
                                .into_any(),
                        );
                    }
                }
            }
            if !listing.recent.is_empty() {
                sections.push(heading("Recent").into_any());
                for run in listing.recent {
                    let title = item_title(app, &run);
                    sections.push(
                        row(
                            app,
                            review,
                            backend.clone(),
                            &build_rows,
                            at,
                            run,
                            Some(title),
                        )
                        .into_any(),
                    );
                }
            }
            Stack::vertical_from_iter(sections)
                .style(|s| s.flex_col().width_full().padding_vert(6.0))
                .into_any()
        },
    )
    .style(|s| s.width_full());

    // The keyboard goes to the first row as the dropdown opens, and the
    // arrows move it along the rows.
    let first = rows.clone();
    let asked = Rc::new(std::cell::Cell::new(false));
    let keys = rows;
    Container::new(
        body.scroll()
            .style(|s| s.width_full().max_height(theme::AGENTS_DROPDOWN_MAX_HEIGHT)),
    )
    .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if !asked.replace(true) {
            if let Some(id) = first.borrow().first() {
                id.request_focus();
            }
        }
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        let step: isize = match key.code {
            Code::ArrowDown => 1,
            Code::ArrowUp => -1,
            _ => return EventPropagation::Continue,
        };
        let rows = keys.borrow();
        if rows.is_empty() {
            return EventPropagation::Stop;
        }
        let next = match at.get_untracked() {
            Some(i) => (i as isize + step).clamp(0, rows.len() as isize - 1) as usize,
            None if step > 0 => 0,
            None => rows.len() - 1,
        };
        rows[next].request_focus();
        EventPropagation::Stop
    })
    .style(|s| {
        s.width(theme::AGENTS_DROPDOWN_WIDTH)
            .flex_col()
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(10.0)
            .box_shadow_blur(22.0)
            .box_shadow_v_offset(7.0)
            .box_shadow_color(theme::SHADOW)
    })
    .debug_name("agents-dropdown")
}

fn heading(text: &'static str) -> impl IntoView {
    Label::derived(move || text.to_uppercase()).style(|s| {
        s.font_size(theme::TEXT_SECTION)
            .font_weight(theme::weight_emphasis())
            .color(theme::FG_SUBTLE)
            .padding_horiz(14.0)
            .padding_top(8.0)
            .padding_bottom(4.0)
            .selectable(false)
    })
}

fn item_heading(title: String) -> impl IntoView {
    Label::derived(move || title.clone()).style(|s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(theme::weight_emphasis())
            .color(theme::FG_MUTED)
            .text_ellipsis()
            .width_full()
            .padding_horiz(14.0)
            .padding_top(6.0)
            .padding_bottom(2.0)
            .selectable(false)
    })
}

/// One run: its status, what it was asked, and under that what ran it and
/// for how long, or, finished, the item it is on and how it ended.
fn row(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    rows: &Rc<RefCell<Vec<floem::ViewId>>>,
    at: RwSignal<Option<usize>>,
    run: AgentRunSummary,
    item: Option<String>,
) -> impl IntoView {
    let index = rows.borrow().len();
    let available = run.source_available
        && run
            .item_id
            .as_deref()
            .is_some_and(|id| app.item(id).is_some());
    let color = status_color(run.status);
    let subject = run.subject();
    let work = match run.mode {
        perch_github::RunMode::Work => "Work · ",
        perch_github::RunMode::Ask => "",
    };
    let who = format!("{work}{}", run.backend_label());
    let status = run.status;
    let queue = queue_position(app, &run.id);
    let waiting_id = run.id.clone();
    let timed = run.clone();
    let detail = move || {
        if !available {
            return "Source unavailable".to_string();
        }
        if let Some(item) = &item {
            return format!("{item} · {}", status.label());
        }
        if review
            .approvals
            .with(|pending| pending.iter().any(|p| p.run_id == waiting_id))
        {
            return format!("{who} · Waiting on you");
        }
        let elapsed = elapsed_label(&timed, app.now.get());
        match (status, queue) {
            (AgentRunStatus::Queued, Some(position)) => {
                let queued_for = elapsed.strip_prefix("Queued ").unwrap_or(&elapsed);
                format!("{who} · Queue #{position} · {queued_for}")
            }
            _ => format!("{who} · {elapsed}"),
        }
    };
    let go = Rc::new({
        let run = run.clone();
        move || go_to_run(app, review, &backend, &run)
    });
    let click = go.clone();
    let view = Stack::horizontal((
        Empty::new().style(move |s| {
            s.size(7.0, 7.0)
                .border_radius(4.0)
                .background(color)
                .margin_top(6.0)
                .flex_shrink(0.0_f32)
        }),
        Stack::vertical((
            Label::derived(move || subject.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG)
                    .text_ellipsis()
                    .width_full()
                    .selectable(false)
            }),
            Label::derived(detail).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::FG_SUBTLE)
                    .text_ellipsis()
                    .width_full()
                    .selectable(false)
            }),
        ))
        .style(|s| s.flex_col().gap(2.0).min_width(0.0).flex_grow(1.0_f32)),
    ))
    .on_event_stop(el::Click, move |_, _| {
        if available {
            click()
        }
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if available && key.code == Code::Enter {
            go();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .on_event_cont(el::FocusGained, move |_, _| at.set(Some(index)))
    .style(move |s| {
        let base = s
            .width_full()
            .items_start()
            .gap(9.0)
            .padding_horiz(14.0)
            .padding_vert(7.0)
            .keyboard_navigable()
            .focus(|s| s.background(theme::CANVAS_HOVER));
        if available {
            clickable(base).hover(|s| s.background(theme::CANVAS_HOVER))
        } else {
            base.opacity(0.6_f32)
        }
    })
    .debug_name(format!("agents-dropdown-run:{}", run.id));
    rows.borrow_mut().push(view.view_id());
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    use floem::headless::HeadlessHarness;

    use crate::review::header::tests::{named, pull_request_harness, rect};

    /// A run on `item`, asked at `minute` past ten.
    fn record(
        id: &str,
        item: &str,
        status: perch_github::RunStatus,
        minute: u32,
    ) -> perch_github::RunRecord {
        let when: jiff::Timestamp = format!("2026-09-19T10:{minute:02}:00Z").parse().unwrap();
        perch_github::RunRecord {
            id: id.into(),
            owner: perch_github::RunOwner::Discussion {
                item_id: item.into(),
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
            ended_at: (status != perch_github::RunStatus::Running).then_some(when),
            ..perch_github::RunRecord::default()
        }
    }

    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..2 {
            harness.rebuild();
            harness.paint();
        }
    }

    fn key(code: Code, name: Key) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: name,
            code,
            location: Default::default(),
            modifiers: Modifiers::empty(),
            repeat: false,
            is_composing: false,
        })
    }

    /// The pull request open, `pr`, and another item, `other`, in the
    /// sidebar, with a run working on each and one finished on `pr`.
    fn window() -> (HeadlessHarness, AppState, ReviewState) {
        let (mut harness, app, review) = pull_request_harness();
        let pr = (*review.item.get().unwrap()).clone();
        let mut other =
            perch_github::Item::new("slatedb/slatedb", "2026-09-20T12:00:00Z".parse().unwrap());
        other.id = "other".into();
        other.name = Some("other-branch".into());
        app.items.set(vec![pr, other]);
        app.open_item.set(Some("pr".into()));
        app.merge_agent_runs(
            [
                record("a", "other", perch_github::RunStatus::Running, 1),
                record("c", "pr", perch_github::RunStatus::Running, 2),
                record("d", "pr", perch_github::RunStatus::Completed, 3),
            ]
            .iter()
            .map(perch_github::RunSummary::from),
            false,
        );
        settle(&mut harness);
        (harness, app, review)
    }

    fn open(harness: &mut HeadlessHarness) {
        let at = rect(harness, "agents-button").center();
        harness.click(at.x, at.y);
        settle(harness);
    }

    /// The button counts every item's working runs and marks one waiting
    /// on the reader. Its dropdown lists the working runs by item and the
    /// ones that finished.
    #[test]
    fn the_button_counts_the_work_and_the_dropdown_lists_it_by_item() {
        let (mut harness, _app, review) = window();
        let count = named(harness.root_id(), "agents count").unwrap();
        assert!(
            count.debug_name().contains("\"2\""),
            "{}",
            count.debug_name()
        );
        assert_eq!(rect(&harness, "agents waiting").width(), 0.0);
        review.approvals.set(vec![crate::backend::ApprovalView {
            run_id: "c".into(),
            id: "1".into(),
            item_id: "pr".into(),
            thread_id: "t-c".into(),
            title: "Connect to example.com".into(),
            command: None,
            detail: None,
            always: String::new(),
        }]);
        settle(&mut harness);
        assert!(
            rect(&harness, "agents waiting").width() > 0.0,
            "no dot for a run waiting"
        );

        open(&mut harness);
        assert!(review.agents_dropdown_open.get());
        let dropdown = rect(&harness, "agents-dropdown");
        let button = rect(&harness, "agents-button");
        assert_eq!(
            dropdown.x1, button.x1,
            "under the button, at its right edge"
        );
        assert!(dropdown.y0 >= button.y1);
        for text in [
            "\"ACTIVE\"",
            "\"RECENT\"",
            "\"pull 812\"",
            "\"other-branch\"",
            "Waiting on you",
            "pull 812 · Completed",
        ] {
            assert!(
                named(harness.root_id(), text).is_some(),
                "the dropdown has no {text}"
            );
        }
        let (a, c, d) = (
            rect(&harness, "agents-dropdown-run:a"),
            rect(&harness, "agents-dropdown-run:c"),
            rect(&harness, "agents-dropdown-run:d"),
        );
        assert!(a.y1 <= d.y0 && c.y1 <= d.y0, "working before finished");
    }

    /// A run on the open item goes to it in place: Conversation, the
    /// Agents row unfolded and the run open.
    #[test]
    fn a_run_on_the_open_item_opens_in_its_agents_row() {
        let (mut harness, _app, review) = window();
        review.show_tab(ItemTab::Code);
        settle(&mut harness);
        open(&mut harness);
        let d = rect(&harness, "agents-dropdown-run:d").center();
        harness.click(d.x, d.y);
        settle(&mut harness);
        assert!(!review.agents_dropdown_open.get());
        assert_eq!(review.tab.get(), ItemTab::Conversation);
        assert!(review.agents_unfolded.get());
        assert!(review.agents_open_runs.with(|open| open.contains("d")));
        assert_eq!(
            review.pending_run_reveal.get(),
            None,
            "the row did not take the ask"
        );
    }

    /// The keyboard is on the first row as the dropdown opens; the arrows
    /// move it and Enter goes to the run.
    #[test]
    fn the_arrows_move_through_the_runs_and_enter_goes_to_one() {
        let (mut harness, _app, review) = window();
        open(&mut harness);
        let row =
            |id: &str| named(harness.root_id(), &format!("agents-dropdown-run:{id}")).unwrap();
        let (a, c, d) = (row("a"), row("c"), row("d"));
        let order = {
            let mut rows = [("a", a), ("c", c), ("d", d)];
            rows.sort_by(|x, y| {
                x.1.get_visual_rect_no_clip()
                    .y0
                    .total_cmp(&y.1.get_visual_rect_no_clip().y0)
            });
            rows
        };
        assert!(
            harness.is_focused(order[0].1),
            "the first row does not have the keyboard"
        );
        harness.dispatch_event(key(Code::ArrowDown, Key::Named(NamedKey::ArrowDown)));
        settle(&mut harness);
        assert!(harness.is_focused(order[1].1));
        harness.dispatch_event(key(Code::ArrowDown, Key::Named(NamedKey::ArrowDown)));
        harness.dispatch_event(key(Code::ArrowDown, Key::Named(NamedKey::ArrowDown)));
        settle(&mut harness);
        assert!(harness.is_focused(order[2].1), "past the last row");
        harness.dispatch_event(key(Code::ArrowUp, Key::Named(NamedKey::ArrowUp)));
        settle(&mut harness);
        assert!(harness.is_focused(order[1].1));
        harness.dispatch_event(key(Code::Enter, Key::Named(NamedKey::Enter)));
        settle(&mut harness);
        assert!(!review.agents_dropdown_open.get());
    }

    /// The dropdown and Submit review shut each other, as Submit review
    /// and the review launcher do.
    #[test]
    fn the_dropdown_and_submit_review_shut_each_other() {
        let (mut harness, _app, review) = window();
        open(&mut harness);
        let submit = rect(&harness, "submit-review-control").center();
        harness.click(submit.x, submit.y);
        settle(&mut harness);
        assert!(review.submit_review_open.get());
        assert!(!review.agents_dropdown_open.get());

        open(&mut harness);
        assert!(review.agents_dropdown_open.get());
        assert!(!review.submit_review_open.get());
    }

    /// The notice a run leaves as it finishes goes to it as a row does.
    #[test]
    fn a_finished_runs_notice_goes_to_it() {
        let (mut harness, app, review) = window();
        review.show_tab(ItemTab::Code);
        app.update_agent_run(&record("c", "pr", perch_github::RunStatus::Completed, 2));
        settle(&mut harness);
        let notice = named(harness.root_id(), "agent-completion-notice").expect("no notice");
        let view = named(notice, "\"View\"").expect("no View on the notice");
        let at = view.get_visual_rect_no_clip().center();
        harness.click(at.x, at.y);
        settle(&mut harness);
        assert_eq!(review.tab.get(), ItemTab::Conversation);
        assert!(review.agents_open_runs.with(|open| open.contains("c")));
    }

    /// A run whose item is gone says so and goes nowhere.
    #[test]
    fn a_run_with_no_source_goes_nowhere() {
        let (mut harness, app, review) = window();
        app.items
            .update(|items| items.retain(|item| item.id != "other"));
        open(&mut harness);
        assert!(named(harness.root_id(), "Source unavailable").is_some());
        let a = rect(&harness, "agents-dropdown-run:a").center();
        harness.click(a.x, a.y);
        settle(&mut harness);
        assert!(
            review.agents_dropdown_open.get(),
            "the click went somewhere"
        );
        assert_eq!(app.open_item.get().as_deref(), Some("pr"));
    }
}
