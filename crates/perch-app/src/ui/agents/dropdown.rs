//! The details button in the bar, and the dropdown it opens: what there
//! is to say about the open item (`review::summary::details`), then the
//! one list of runs, under Agents. With nothing open the button is
//! Agents, and the dropdown the runs alone. What is working comes first,
//! the open item's before the other items', each turning a spinner where
//! a finished run has a dot. Then what finished, newest first: the open
//! item's, a review's reviewers as one row for the review and older runs
//! a row away, then what finished last elsewhere. Another item's run
//! names its item.
//!
//! A question's run goes to its thread on Conversation, where its answer
//! is; a reviewer's run, or a review's row, to the review in the Review
//! popover, where its findings are. Under the pointer a row offers Stop
//! while its run works and Retry once it failed.
//!
//! The button sits before the item's one action, where Review opens its
//! popover, and on the pane with nothing open, so the runs can be reached
//! from anywhere. Its count is every item's running and queued work, and a
//! dot says a run is waiting on the reader.

use std::cell::RefCell;
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, ScrollExt, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::cards::{hover_action, tip_with, track_hover};
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, icon_in};

use super::model::{
    active_groups_with_reviews, elapsed_label, group_review_runs, history_runs_with_reviews,
    item_runs, with_every_review, AgentReviewRunGroup, AgentRunEntry, AgentRunStatus,
    AgentRunSummary,
};
use super::{queue_position, reviews, run_actions, status_color};

/// How many finished runs the dropdown lists.
pub const RECENT_RUNS: usize = 15;

/// The button, and the dropdown under it while it is open.
pub fn button(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let count = move || {
        let (running, queued) = app.active_agent_counts();
        running + queued
    };
    let waiting = move || review.approvals.with(|pending| !pending.is_empty());
    // The glyph alone, as the gauge beside it, with the count and the dot
    // pinned to its corners.
    let button = Stack::new((
        // A list while an item is open, for its details over the runs;
        // the agents' own glyph with nothing open.
        icon_in(
            move || {
                let glyph = if review.item.with(Option::is_some) {
                    SidebarIcon::BulletList
                } else {
                    SidebarIcon::Agents
                };
                crate::icons::sidebar_icon(glyph, theme::fg_muted())
            },
            theme::fg_muted,
        )
        .style(|s| s.pointer_events_none()),
        Label::derived(move || count().to_string())
            .debug_name("agents count")
            .style(move |s| {
                s.position(Position::Absolute)
                    .inset_top(-3.0)
                    .inset_right(-5.0)
                    .font_size(10.0)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg_muted())
                    .height(15.0)
                    .min_width(15.0)
                    .padding_horiz(3.0)
                    .border(1.5)
                    .border_color(theme::canvas())
                    .border_radius(7.5)
                    .background(theme::badge_bg())
                    .items_center()
                    .justify_center()
                    .selectable(false)
                    .pointer_events_none()
                    .apply_if(count() == 0, |s| s.hide())
            }),
        Empty::new().debug_name("agents waiting").style(move |s| {
            s.position(Position::Absolute)
                .inset_bottom(1.0)
                .inset_right(1.0)
                .size(8.0, 8.0)
                .border(1.5)
                .border_color(theme::canvas())
                .border_radius(4.0)
                .background(theme::accent())
                .pointer_events_none()
                .apply_if(!waiting(), |s| s.hide())
        }),
    ));
    let button_id = button.view_id();
    let click_backend = backend.clone();
    let key_backend = backend.clone();
    let button = button
        .on_event_stop(el::Click, move |_, _| {
            toggle(review, &click_backend, Some(button_id))
        })
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Enter | Code::Space) {
                toggle(review, &key_backend, Some(button_id));
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(move |s| {
            clickable(s)
                .size(26.0, 26.0)
                .items_center()
                .justify_center()
                .border_radius(13.0)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
                .hover(|s| s.background(theme::badge_bg()))
                .apply_if(review.agents_dropdown_open.get(), |s| {
                    s.background(theme::badge_bg())
                })
        })
        .debug_name("agents-button");

    let stack = Stack::new((
        tip_with(button, move || {
            if review.item.with_untracked(Option::is_some) {
                "Details and agents".to_string()
            } else {
                "Agents".to_string()
            }
        }),
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
                .inset_top(32.0)
                .inset_right(0.0)
                .z_index(100)
        }),
    ));
    // A press outside the button and what hangs from it shuts it: see
    // `ui::dismiss_popovers`.
    let anchor = stack.view_id();
    Effect::new(move |_| {
        if review.agents_dropdown_open.get() {
            review.agents_dropdown_anchor.set(Some(anchor));
        }
    });
    stack
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
pub fn toggle(review: ReviewState, backend: &Backend, opener: Option<floem::ViewId>) {
    if review.agents_dropdown_open.get_untracked() {
        close(review, true);
    } else {
        open(review, backend, opener);
    }
}

/// Open the dropdown over whatever else hangs from the bar, and ask for
/// the runs it lists.
pub fn open(review: ReviewState, backend: &Backend, opener: Option<floem::ViewId>) {
    crate::review::review_menu::close(review, false);
    crate::ui::usage::close(review, false);
    crate::ui::feedback::close(review, false);
    review.agents_dropdown_opener.set(opener);
    review.agents_dropdown_open.set(true);
    backend.load_agent_runs(RECENT_RUNS);
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

/// Go to a run where it is read: see `super::reveal`. A run on the item
/// open is brought into view in place; another item is opened first, and
/// the run brought into view once its discussion has landed.
pub fn go_to_run(app: AppState, review: ReviewState, backend: &Backend, run: &AgentRunSummary) {
    close(review, false);
    let Some(item_id) = run
        .item_id
        .clone()
        .filter(|item_id| run.source_available && app.item(item_id).is_some())
    else {
        app.complain("The source for this run is no longer available");
        return;
    };
    let same_item = app
        .open_item
        .with_untracked(|open| open.as_deref() == Some(item_id.as_str()))
        && review.is_showing();
    // A retained snapshot of the item is not where its threads are: the
    // item is opened again, as it is.
    if same_item && review.historical_review.with_untracked(Option::is_none) {
        review.pending_run_reveal.set(Some(run.id.clone()));
        return;
    }
    let navigation_id = backend.view_agent_owner(&item_id);
    app.begin_agent_run_navigation(item_id, run.id.clone(), navigation_id);
    app.begin_review(review, "Reading the discussion");
}

/// The open item's runs.
#[derive(Clone, PartialEq)]
struct ThisItem {
    id: String,
    entries: Vec<AgentRunEntry>,
    /// Whether older runs are left to load, and whether a page is on its
    /// way.
    page: Option<(bool, bool)>,
}

/// What the dropdown lists: the open item's runs, what is working on
/// the other items, and the runs that finished last on them.
#[derive(Clone, PartialEq)]
struct Listing {
    this: Option<ThisItem>,
    working: Vec<AgentRunSummary>,
    finished: Vec<AgentRunSummary>,
}

fn listing(app: AppState, review: ReviewState) -> Listing {
    let review_groups = app.review_groups.with(|all| {
        all.values()
            .flat_map(|groups| groups.iter().cloned())
            .collect::<Vec<_>>()
    });
    let this_id = review
        .item
        .with(|item| item.as_ref().map(|item| item.id.clone()));
    let elsewhere = |run: &AgentRunSummary| this_id.is_none() || run.item_id != this_id;
    app.agent_runs.with(|runs| {
        let this = this_id.clone().map(|id| {
            let groups = app.review_groups_for(&id);
            let entries = with_every_review(
                group_review_runs(&item_runs(runs, &id, &groups), &groups),
                &groups,
            );
            let page = app
                .item_runs
                .with(|pages| pages.get(&id).map(|page| (page.has_more, page.loading)));
            ThisItem { id, entries, page }
        });
        let working = active_groups_with_reviews(runs, None, &review_groups)
            .into_iter()
            .flat_map(|group| group.runs)
            .filter(|run| elsewhere(run))
            .collect();
        let finished = history_runs_with_reviews(runs, &review_groups)
            .into_iter()
            .filter(|run| elsewhere(run))
            .take(RECENT_RUNS)
            .collect();
        Listing {
            this,
            working,
            finished,
        }
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

/// The rows in the order they are drawn, for the arrow keys to move
/// through, and which one has the keyboard.
#[derive(Clone)]
struct Rows {
    ids: Rc<RefCell<Vec<floem::ViewId>>>,
    at: RwSignal<Option<usize>>,
    /// Whether the arrows have been used. The first row has the keyboard
    /// from the start, but is lit for it only once the keys are in use,
    /// so a click that opens the dropdown doesn't light a row it isn't on.
    keyed: RwSignal<bool>,
}

impl Rows {
    /// Note a row as the next in order, and follow it taking the keyboard.
    fn add<V: IntoView + 'static>(&self, view: V) -> impl IntoView {
        let view = view.into_intermediate();
        let index = {
            let mut ids = self.ids.borrow_mut();
            ids.push(view.view_id());
            ids.len() - 1
        };
        let (at, keyed) = (self.at, self.keyed);
        view.on_event_cont(el::FocusGained, move |_, _| at.set(Some(index)))
            .style(move |s| {
                s.apply_if(keyed.get(), |s| {
                    s.focus(|s| s.background(theme::canvas_hover()))
                })
            })
    }
}

fn popover(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let rows = Rows {
        ids: Rc::new(RefCell::new(Vec::new())),
        at: RwSignal::new(None),
        keyed: RwSignal::new(false),
    };
    // The open item's runs, the first page, the first time it is listed.
    if let Some(id) = review
        .item
        .with_untracked(|item| item.as_ref().map(|item| item.id.clone()))
    {
        super::load_item_runs(app, &backend, &id);
    }
    let listing = Memo::new(move |_| listing(app, review));
    let details = crate::review::summary::details(app, review, backend.clone());
    let build_rows = rows.clone();
    let body = dyn_container(
        move || listing.get(),
        move |listing| {
            build_rows.ids.borrow_mut().clear();
            let rows = &build_rows;
            let this_empty = listing.this.as_ref().is_none_or(|this| {
                this.entries.is_empty() && this.page.is_none_or(|(more, _)| !more)
            });
            if this_empty && listing.working.is_empty() && listing.finished.is_empty() {
                return Label::derived(|| "No agent runs yet".to_string())
                    .style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::fg_subtle())
                            .padding(16.0)
                            .selectable(false)
                    })
                    .into_any();
            }
            let mut sections = vec![heading("Agents").into_any()];
            let other = |run: AgentRunSummary| {
                let title = item_title(app, &run);
                row(app, review, backend.clone(), rows, run, Some(title)).into_any()
            };
            // What is working, the open item's first; then what finished,
            // its own again first.
            let (id, page, (this_working, this_finished)) = match listing.this {
                Some(this) => (
                    Some(this.id),
                    this.page,
                    this.entries.into_iter().partition(is_working),
                ),
                None => (None, None, (Vec::new(), Vec::new())),
            };
            let mine = |entries: Vec<AgentRunEntry>, sections: &mut Vec<floem::AnyView>| {
                for entry in entries {
                    sections.push(match entry {
                        AgentRunEntry::Run(run) => {
                            row(app, review, backend.clone(), rows, run, None).into_any()
                        }
                        AgentRunEntry::ReviewGroup(group) => {
                            review_row(app, review, backend.clone(), rows, group).into_any()
                        }
                    });
                }
            };
            mine(this_working, &mut sections);
            sections.extend(listing.working.into_iter().map(other));
            mine(this_finished, &mut sections);
            if let (Some(id), Some((true, loading))) = (id, page) {
                sections.push(older(app, backend.clone(), rows, id, loading).into_any());
            }
            sections.extend(listing.finished.into_iter().map(other));
            Stack::vertical_from_iter(sections)
                .style(|s| s.flex_col().width_full().padding_vert(6.0))
                .into_any()
        },
    )
    .style(|s| s.width_full().max_width_full());

    // The keyboard goes to the first row as the dropdown opens, and the
    // arrows move it along the rows.
    let first = rows.ids.clone();
    let asked = Rc::new(std::cell::Cell::new(false));
    let keys = rows.clone();
    Container::new(
        Stack::vertical((details, body))
            .style(|s| s.flex_col().width_full().max_width_full())
            .scroll()
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
        if !keys.keyed.get_untracked() {
            keys.keyed.set(true);
        }
        let rows = keys.ids.borrow();
        if rows.is_empty() {
            return EventPropagation::Stop;
        }
        let next = match keys.at.get_untracked() {
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
            .apply(crate::ui::menu::panel_surface(floem::style::Style::new()))
    })
    .debug_name("agents-dropdown")
}

/// Whether one of the open item's rows is working: its run, or any of a
/// review's reviewers.
fn is_working(entry: &AgentRunEntry) -> bool {
    match entry {
        AgentRunEntry::Run(run) => run.status.is_active(),
        AgentRunEntry::ReviewGroup(group) => reviews::group_status(group).is_active(),
    }
}

fn heading(text: &'static str) -> impl IntoView {
    Label::derived(move || text.to_uppercase()).style(|s| {
        s.font_size(theme::TEXT_SECTION)
            .font_weight(theme::weight_emphasis())
            .color(theme::fg_subtle())
            .padding_horiz(14.0)
            .padding_top(8.0)
            .padding_bottom(4.0)
            .selectable(false)
    })
}

/// A row's frame: a spinner while it works or a dot for how it ended, two
/// lines beside it, and what can be done with it under the pointer. A press or Enter runs `go`
/// while it is `available`.
#[allow(clippy::too_many_arguments)]
fn frame(
    rows: &Rows,
    name: String,
    status: AgentRunStatus,
    subject: impl Fn() -> String + 'static,
    detail: impl Fn() -> String + 'static,
    actions: impl FnOnce(RwSignal<bool>) -> floem::AnyView,
    available: bool,
    go: Rc<dyn Fn()>,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let click = go.clone();
    // Both marks fill a 12px column, the dot centred in it.
    let mark = if status.is_active() {
        crate::review::run_view::spinner(theme::running)
            .style(|s| s.size(12.0, 12.0).margin_top(3.0).flex_shrink(0.0_f32))
            .into_any()
    } else {
        let color = status_color(status);
        Empty::new()
            .style(move |s| {
                s.size(7.0, 7.0)
                    .border_radius(4.0)
                    .background(color())
                    .margin_top(6.0)
                    .margin_left(2.0)
                    .margin_right(3.0)
                    .flex_shrink(0.0_f32)
            })
            .into_any()
    };
    let view = Stack::horizontal((
        mark,
        Stack::vertical((
            Label::derived(subject).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg())
                    .text_ellipsis()
                    .width_full()
                    .selectable(false)
            }),
            Label::derived(detail).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .text_ellipsis()
                    .width_full()
                    .selectable(false)
            }),
        ))
        .style(|s| {
            s.flex_col()
                .gap(2.0)
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
        Container::new(actions(hovered)).style(|s| s.flex_shrink(0.0_f32).items_center()),
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
    .debug_name(name)
    .style(move |s| {
        let base = s
            .width_full()
            .items_start()
            .gap(6.0)
            .padding_left(12.0)
            .padding_right(8.0)
            .padding_vert(7.0)
            .keyboard_navigable();
        if available {
            clickable(base).hover(|s| s.background(theme::canvas_hover()))
        } else {
            base.opacity(0.6_f32)
        }
    });
    track_hover(rows.add(view), hovered)
}

/// One run: its status, what it was asked, and under that what ran it and
/// for how long, or, on another item, that item and how long the run has
/// worked or how it ended.
fn row(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    rows: &Rows,
    run: AgentRunSummary,
    item: Option<String>,
) -> impl IntoView {
    let available = run.source_available
        && run
            .item_id
            .as_deref()
            .is_some_and(|id| app.item(id).is_some());
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
        let waiting = review
            .approvals
            .with(|pending| pending.iter().any(|p| p.run_id == waiting_id));
        if let Some(item) = &item {
            return if waiting {
                format!("{item} · Waiting on you")
            } else if status.is_active() {
                format!("{item} · {}", elapsed_label(&timed, app.now.get()))
            } else {
                format!("{item} · {}", status.label())
            };
        }
        if waiting {
            return format!("{who} · Waiting on you");
        }
        let elapsed = elapsed_label(&timed, app.now.get());
        match (status, queue) {
            (AgentRunStatus::Queued, Some(position)) => {
                let queued_for = elapsed.strip_prefix("Queued ").unwrap_or(&elapsed);
                format!("{who} · Queue #{position} · {queued_for}")
            }
            _ if !status.is_active() => format!("{who} · {} · {elapsed}", status.label()),
            _ => format!("{who} · {elapsed}"),
        }
    };
    let actions_backend = backend.clone();
    let go: Rc<dyn Fn()> = Rc::new({
        let run = run.clone();
        move || go_to_run(app, review, &backend, &run)
    });
    frame(
        rows,
        format!("agents-dropdown-run:{}", run.id),
        status,
        move || subject.clone(),
        detail,
        move |hovered| run_actions(hovered, actions_backend, &run).into_any(),
        available,
        go,
    )
}

/// One of the open item's reviews, as one row for its reviewers: which
/// review it was, and who ran it and what it found or how it runs. It goes
/// to the review in the Review popover.
fn review_row(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    rows: &Rows,
    group: AgentReviewRunGroup,
) -> impl IntoView {
    let status = reviews::group_status(&group);
    // Which review and who ran it, over how it stands: a commit's hash
    // says nothing a reader goes by.
    let subject = [
        group.summary.as_ref().map_or_else(
            || "Review".to_string(),
            |summary| format!("Review #{}", summary.group.number),
        ),
        reviews::reviewers_label(&group),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" \u{b7} ");
    let detail_group = group.clone();
    let detail = move || reviews::how_label(&detail_group, app.now.get());
    let retry = reviews::retryable_attempts(&group);
    let stop = reviews::stoppable(&group);
    let actions = move |hovered: RwSignal<bool>| -> floem::AnyView {
        if let Some(group_id) = stop {
            hover_action(
                hovered,
                SidebarIcon::Stop,
                "Stop review",
                "agent-review stop",
                Rc::new(move || backend.stop_review_group(&group_id)),
            )
            .into_any()
        } else if !retry.is_empty() {
            hover_action(
                hovered,
                SidebarIcon::Refresh,
                "Retry",
                "agent-review retry",
                Rc::new(move || {
                    for attempt in &retry {
                        backend.retry_review_attempt(attempt);
                    }
                }),
            )
            .into_any()
        } else {
            Empty::new().into_any()
        }
    };
    let key = group.key.clone();
    let go: Rc<dyn Fn()> = Rc::new(move || {
        close(review, false);
        super::open_review(review, key.clone());
    });
    frame(
        rows,
        format!("agents-dropdown-review:{}", group.key),
        status,
        move || subject.clone(),
        detail,
        actions,
        true,
        go,
    )
}

/// Load older runs of the open item, while it has more.
fn older(
    app: AppState,
    backend: Rc<Backend>,
    rows: &Rows,
    item_id: String,
    loading: bool,
) -> impl IntoView {
    let go = Rc::new(move || {
        if !loading {
            app.load_item_runs(&backend, &item_id, true);
        }
    });
    let key_go = go.clone();
    let view = Label::derived(move || {
        if loading {
            "Loading…".to_string()
        } else {
            "Load older runs".to_string()
        }
    })
    .on_event_stop(el::Click, move |_, _| go())
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Enter {
            key_go();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .debug_name("agents-dropdown-older")
    .style(|s| {
        clickable(s)
            .width_full()
            .padding_horiz(30.0)
            .padding_vert(7.0)
            .font_size(theme::TEXT_BADGE)
            .color(theme::fg_muted())
            .selectable(false)
            .keyboard_navigable()
            .hover(|s| s.background(theme::canvas_hover()))
    });
    rows.add(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    use floem::headless::HeadlessHarness;
    use perch_github::RunStatus;

    use crate::backend::Command;
    use crate::review::doc::CardId;
    use crate::review::model::ItemTab;
    use crate::ui::agents::test_support::{
        add, click, named, record, rect, review as review_summary, reviewer, sent, set_reviews,
        settle, shown, thread, window as recording_window,
    };

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
    /// sidebar: a run working on each, one finished on `pr` and one
    /// finished on `other`.
    fn window() -> (
        HeadlessHarness,
        AppState,
        ReviewState,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) {
        let (mut harness, app, review, commands) = recording_window();
        let pr = (*review.item.get().unwrap()).clone();
        let mut other =
            perch_github::Item::new("slatedb/slatedb", "2026-09-20T12:00:00Z".parse().unwrap());
        other.id = "other".into();
        other.name = Some("other-branch".into());
        app.items.set(vec![pr, other]);
        add(
            app,
            &[
                record("a", "other", RunStatus::Running, 1),
                record("c", "pr", RunStatus::Running, 2),
                record("d", "pr", RunStatus::Completed, 3),
                record("e", "other", RunStatus::Completed, 4),
            ],
        );
        settle(&mut harness);
        (harness, app, review, commands)
    }

    fn open(harness: &mut HeadlessHarness) {
        click(harness, "agents-button");
    }

    fn y(harness: &HeadlessHarness, name: &str) -> f64 {
        rect(harness, name).y0
    }

    /// The button counts every item's working runs and marks one waiting
    /// on the reader. Its dropdown is one list under Agents: what works,
    /// the open pull request's first, then what finished, the pull
    /// request's first and a review's reviewers as one row for the
    /// review. A working run's row turns a spinner.
    #[test]
    fn the_dropdown_is_one_list_working_first_and_this_items_before_the_others() {
        let (mut harness, app, review, _) = window();
        add(
            app,
            &[
                reviewer("claude-run", "attempt-claude", RunStatus::Completed, 5),
                reviewer("codex-run", "attempt-codex", RunStatus::Completed, 6),
            ],
        );
        set_reviews(
            app,
            vec![review_summary(3, &["attempt-claude", "attempt-codex"])],
        );
        settle(&mut harness);
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
            "\"AGENTS\"",
            "other-branch · ",
            "other-branch · Completed",
            "Waiting on you",
        ] {
            assert!(
                named(harness.root_id(), text).is_some(),
                "the dropdown has no {text}"
            );
        }
        for text in [
            "\"THIS PULL REQUEST\"",
            "\"RUNNING ELSEWHERE\"",
            "\"ACTIVE\"",
            "\"RECENT\"",
        ] {
            assert!(
                named(harness.root_id(), text).is_none(),
                "the dropdown still has {text}"
            );
        }
        for (row, spins) in [
            ("agents-dropdown-run:a", true),
            ("agents-dropdown-run:c", true),
            ("agents-dropdown-run:d", false),
            ("agents-dropdown-run:e", false),
        ] {
            assert_eq!(
                named(rect_id(&harness, row), "spinner").is_some(),
                spins,
                "{row}'s mark"
            );
        }
        // Working, the pull request's first; then finished, the pull
        // request's newest first with the review one row for its two
        // reviewers, then the other item's.
        let this = [
            "agents-dropdown-run:c",
            "agents-dropdown-run:a",
            "agents-dropdown-review:group-3",
            "agents-dropdown-run:d",
            "agents-dropdown-run:e",
        ];
        for pair in this.windows(2) {
            assert!(
                y(&harness, pair[0]) < y(&harness, pair[1]),
                "{} is not over {}",
                pair[0],
                pair[1]
            );
        }
        assert!(named(harness.root_id(), "agents-dropdown-run:claude-run").is_none());
        assert!(named(harness.root_id(), "Review #3 · Claude").is_some());
        assert!(named(harness.root_id(), "\"nothing found\"").is_some());
        // The pull request's run is not listed again with the others'.
        let mut rows = Vec::new();
        fn all(id: floem::ViewId, rows: &mut Vec<String>) {
            if id.debug_name().contains("agents-dropdown-run:d") {
                rows.push(id.debug_name().to_string());
            }
            for child in id.children() {
                all(child, rows);
            }
        }
        all(rect_id(&harness, "agents-dropdown"), &mut rows);
        assert_eq!(rows.len(), 1, "the pull request's run is listed twice");
        assert!(named(harness.root_id(), "pull 812 · Completed").is_none());
    }

    fn rect_id(harness: &HeadlessHarness, name: &str) -> floem::ViewId {
        named(harness.root_id(), name).unwrap()
    }

    /// With no item open the dropdown is the one list: what works, then
    /// what finished.
    #[test]
    fn with_nothing_open_the_dropdown_is_one_list() {
        let (mut harness, _app, review, _) = window();
        click(&mut harness, "close button");
        assert!(review.item.get().is_none());
        open(&mut harness);
        for text in ["\"AGENTS\"", "pull 812 · Completed"] {
            assert!(
                named(harness.root_id(), text).is_some(),
                "the dropdown has no {text}"
            );
        }
        for text in ["\"THIS PULL REQUEST\"", "\"ACTIVE\"", "\"RECENT\""] {
            assert!(
                named(harness.root_id(), text).is_none(),
                "the dropdown still has {text}"
            );
        }
        let rows = ["c", "a", "d", "e"].map(|run| format!("agents-dropdown-run:{run}"));
        assert!(y(&harness, &rows[0]).max(y(&harness, &rows[1])) < y(&harness, &rows[2]));
        assert!(y(&harness, &rows[0]).max(y(&harness, &rows[1])) < y(&harness, &rows[3]));
    }

    /// Under the pointer, a working run's row stops it and a failed
    /// one's runs it again; a review's row stops or retries the review.
    #[test]
    fn stop_and_retry_under_the_pointer_go_to_the_backend() {
        let (mut harness, app, _review, mut commands) = window();
        add(
            app,
            &[
                record("f", "pr", RunStatus::Failed, 5),
                reviewer("claude-run", "attempt-claude", RunStatus::Failed, 6),
            ],
        );
        let mut failed = review_summary(3, &["attempt-claude"]);
        failed.group.status = perch_github::ReviewGroupStatus::Failed;
        failed.attempts[0].phase = perch_github::ReviewerPhase::Failed;
        set_reviews(app, vec![failed]);
        open(&mut harness);
        sent(&mut commands);

        let press = |harness: &mut HeadlessHarness, row: &str, action: &str| {
            let at = rect(harness, row);
            harness.pointer_move(at.x0 + 40.0, at.center().y);
            settle(harness);
            let action = named(rect_id(harness, row), action)
                .unwrap_or_else(|| panic!("{row} has no {action}"))
                .get_visual_rect_no_clip();
            assert!(action.height() > 0.0, "{row}'s action is not drawn");
            harness.click(action.center().x, action.center().y);
            settle(harness);
        };
        press(&mut harness, "agents-dropdown-run:c", "agent-run stop");
        assert!(
            sent(&mut commands)
                .iter()
                .any(|c| matches!(c, Command::CancelRun { run_id } if run_id == "c")),
            "Stop did not cancel the run"
        );
        press(&mut harness, "agents-dropdown-run:f", "agent-run retry");
        assert!(
            sent(&mut commands)
                .iter()
                .any(|c| matches!(c, Command::RetryRun { run_id } if run_id == "f")),
            "Retry did not run it again"
        );
        press(
            &mut harness,
            "agents-dropdown-review:group-3",
            "agent-review retry",
        );
        assert!(
            sent(&mut commands).iter().any(|c| matches!(
                c,
                Command::RetryReviewAttempt { attempt_id } if attempt_id == "attempt-claude"
            )),
            "Retry did not run the review's reviewer again"
        );
        assert!(
            _review.agents_dropdown_open.get(),
            "an action went to the run"
        );
    }

    /// A question's run with no thread to go to puts up what it did in
    /// its sheet instead, and Escape puts the sheet away without reaching
    /// anything under it.
    #[test]
    fn a_question_without_a_thread_opens_its_output() {
        let (mut harness, app, review, _) = window();
        let mut run = AgentRunSummary::from_summary(&perch_github::RunSummary::from(&record(
            "q",
            "pr",
            RunStatus::Completed,
            7,
        )));
        run.thread_id = None;
        app.agent_runs.update(|runs| {
            let mut all = (**runs).clone();
            all.push(run);
            *runs = std::sync::Arc::new(all);
        });
        settle(&mut harness);
        open(&mut harness);
        click(&mut harness, "agents-dropdown-run:q");
        settle(&mut harness);
        assert!(!review.agents_dropdown_open.get());
        assert_eq!(review.agent_output.get().as_deref(), Some("q"));
        assert!(shown(&harness, "agent-output-panel"), "the sheet is not up");
        assert_eq!(review.focus_card.get(), None);
        let popover = review.review_open.get();

        harness.dispatch_event(key(Code::Escape, Key::Named(NamedKey::Escape)));
        settle(&mut harness);
        assert_eq!(review.agent_output.get(), None);
        assert!(!shown(&harness, "agent-output-panel"));
        assert_eq!(review.review_open.get(), popover, "Escape reached under it");
    }

    /// A question's run on the open item goes to its thread on
    /// Conversation, which comes into view.
    #[test]
    fn a_question_goes_to_its_thread() {
        let (mut harness, _app, review, _) = window();
        review.set_comment_threads(std::sync::Arc::new(vec![thread(
            "t-d",
            "Why the fence?",
            4,
        )]));
        review.show_tab(ItemTab::Code);
        settle(&mut harness);
        open(&mut harness);
        click(&mut harness, "agents-dropdown-run:d");
        settle(&mut harness);
        assert!(!review.agents_dropdown_open.get());
        assert_eq!(review.tab.get(), ItemTab::Conversation);
        assert_eq!(review.pending_run_reveal.get(), None);
        assert_eq!(
            review.pending_focus_card.get(),
            None,
            "the card never took it"
        );
        assert_eq!(review.focus_card.get(), Some(CardId::Thread("t-d".into())));
        let card = named(harness.root_id(), "Why the fence?")
            .expect("the thread is not on the page")
            .get_visual_rect_no_clip();
        let page = rect(&harness, "conversation column");
        assert!(
            card.height() > 0.0 && card.y0 < page.y1,
            "the thread is not in view"
        );
    }

    /// A review's row, or a reviewer's run, goes to the review in the
    /// Review popover: open there, and the reader's own review folded.
    #[test]
    fn a_review_opens_the_review_popover_at_it() {
        let (mut harness, app, review, _) = window();
        add(
            app,
            &[reviewer(
                "claude-run",
                "attempt-claude",
                RunStatus::Completed,
                5,
            )],
        );
        set_reviews(
            app,
            vec![
                review_summary(2, &[]),
                review_summary(3, &["attempt-claude"]),
            ],
        );
        settle(&mut harness);
        open(&mut harness);
        click(&mut harness, "agents-dropdown-review:group-2");
        assert!(!review.agents_dropdown_open.get());
        assert!(review.review_open.get());
        assert_eq!(review.review_expanded.get().as_deref(), Some("group-2"));
        assert!(
            shown(&harness, "submit-review-folded"),
            "your review is open"
        );
        let body = named(
            rect_id(&harness, "agent-review:group-2"),
            "agent-review-body",
        );
        assert!(
            body.is_some_and(|body| body.get_visual_rect_no_clip().height() > 0.0),
            "the review is not open"
        );
        assert!(review.review_older_shown.get(), "an older review is hidden");

        // A reviewer's run, from wherever it is asked for.
        crate::review::review_menu::close(review, false);
        settle(&mut harness);
        review.pending_run_reveal.set(Some("claude-run".into()));
        settle(&mut harness);
        assert!(review.review_open.get());
        assert_eq!(review.review_expanded.get().as_deref(), Some("group-3"));
    }

    /// Older runs of the open item load a page at a time while it has more.
    #[test]
    fn older_runs_load_while_the_item_has_more() {
        let (mut harness, app, _review, _) = window();
        app.item_runs.update(|pages| {
            pages.insert(
                "pr".into(),
                crate::state::HistoryPage {
                    has_more: true,
                    loading: false,
                },
            );
        });
        open(&mut harness);
        assert!(named(harness.root_id(), "\"Load older runs\"").is_some());
        click(&mut harness, "agents-dropdown-older");
        assert!(app.item_runs.with(|pages| pages["pr"].loading));
        assert!(named(harness.root_id(), "\"Loading…\"").is_some());
    }

    /// The keyboard is on the first row as the dropdown opens; the arrows
    /// move it and Enter goes to the run.
    #[test]
    fn the_arrows_move_through_the_runs_and_enter_goes_to_one() {
        let (mut harness, _app, review, _) = window();
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
        settle(&mut harness);
        assert!(harness.is_focused(order[2].1));
        harness.dispatch_event(key(Code::ArrowUp, Key::Named(NamedKey::ArrowUp)));
        settle(&mut harness);
        assert!(harness.is_focused(order[1].1));
        harness.dispatch_event(key(Code::Enter, Key::Named(NamedKey::Enter)));
        settle(&mut harness);
        assert!(!review.agents_dropdown_open.get());
    }

    /// The dropdown and Review shut each other.
    #[test]
    fn the_dropdown_and_review_shut_each_other() {
        let (mut harness, _app, review, _) = window();
        open(&mut harness);
        click(&mut harness, "review-control");
        assert!(review.review_open.get());
        assert!(!review.agents_dropdown_open.get());

        open(&mut harness);
        assert!(review.agents_dropdown_open.get());
        assert!(!review.review_open.get());
    }

    /// The toast a run leaves as it finishes goes to it as a row does,
    /// from a click anywhere on the card but its close button.
    #[test]
    fn a_finished_runs_toast_goes_to_it() {
        let (mut harness, app, review, _) = window();
        review.set_comment_threads(std::sync::Arc::new(vec![thread("t-c", "Why the lock?", 4)]));
        review.show_tab(ItemTab::Code);
        app.update_agent_run(&record("c", "pr", RunStatus::Completed, 2));
        settle(&mut harness);
        let toast = named(harness.root_id(), "toast").expect("no toast");
        assert!(named(toast, "\"View\"").is_none(), "a View button");
        let close = named(toast, "Dismiss agent completion").expect("no close button");
        let card = toast.get_visual_rect_no_clip();
        let close = close.get_visual_rect_no_clip();
        // The blank strip between the text and the close button.
        harness.click(close.x0 - 6.0, card.center().y);
        settle(&mut harness);
        assert_eq!(review.tab.get(), ItemTab::Conversation);
        assert_eq!(review.focus_card.get(), Some(CardId::Thread("t-c".into())));
        assert!(app.current_toast().is_none());
    }

    /// Closing a run's toast takes it down and goes nowhere.
    #[test]
    fn closing_a_finished_runs_toast_stays_put() {
        let (mut harness, app, review, _) = window();
        review.show_tab(ItemTab::Code);
        app.update_agent_run(&record("c", "pr", RunStatus::Completed, 2));
        settle(&mut harness);
        let toast = named(harness.root_id(), "toast").expect("no toast");
        let close = named(toast, "Dismiss agent completion").expect("no close button");
        let at = close.get_visual_rect_no_clip().center();
        harness.click(at.x, at.y);
        settle(&mut harness);
        assert!(app.current_toast().is_none());
        assert_eq!(review.tab.get(), ItemTab::Code);
    }

    /// A run whose item is gone says so and goes nowhere.
    #[test]
    fn a_run_with_no_source_goes_nowhere() {
        let (mut harness, app, review, _) = window();
        app.items
            .update(|items| items.retain(|item| item.id != "other"));
        open(&mut harness);
        assert!(named(harness.root_id(), "Source unavailable").is_some());
        click(&mut harness, "agents-dropdown-run:a");
        assert!(
            review.agents_dropdown_open.get(),
            "the click went somewhere"
        );
        assert_eq!(app.open_item.get().as_deref(), Some("pr"));
    }
}
