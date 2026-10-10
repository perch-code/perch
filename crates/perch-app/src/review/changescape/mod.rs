//! The Changescape tab: Changescape's tour of a pull request, drawn from
//! the `page.json` it left in its cache. The tour walks the change as
//! chapters of stops, each stop one idea with the code that carries it.
//! A way into Perch, not a second code viewer: its code is cut from the
//! files Code has, a click opens Code there, and its threads are Perch's.
//!
//! With no changescape for the item, nothing here shows: no pill, no empty
//! state.

pub mod hunk;
pub mod navigate;
pub mod prose;
pub mod side;
pub mod stop;
pub mod strip;
pub mod svg;
pub mod tour;
pub mod view;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use perch_changescape::{LoadedPage, Lookup, StopIndex};

use crate::backend::changescape::Found;
use crate::backend::Backend;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;

/// What the backend found for one pull request at one head, with the
/// stop index built once for it.
#[derive(Debug)]
pub struct Shown {
    pub item_id: String,
    pub found: Arc<Found>,
    pub index: StopIndex,
}

impl Shown {
    pub fn new(item_id: String, found: Arc<Found>) -> Shown {
        let index = found
            .page
            .as_deref()
            .map(StopIndex::new)
            .unwrap_or_default();
        Shown {
            item_id,
            found,
            index,
        }
    }

    pub fn page(&self) -> Option<&Arc<LoadedPage>> {
        self.found.page.as_ref()
    }

    pub fn lookup(&self) -> &Lookup {
        &self.found.lookup
    }

    /// Whether the tab shows: a page, a running build, a page this Perch
    /// could not read, or a build this Perch can start or could with a
    /// newer skill.
    pub fn has_tab(&self) -> bool {
        self.found.page.is_some()
            || self.found.lookup.is_running()
            || self.unreadable()
            || self.found.can_build
            || self.found.skill_too_old
    }

    /// A finished page that did not read: a bug in the page's producer.
    pub fn unreadable(&self) -> bool {
        self.found.page.is_none() && self.found.lookup.drawable().is_some()
    }

    /// The page is the current head's facts, and the build this Perch
    /// runs is still writing its narration.
    pub fn narration_pending(&self) -> bool {
        self.found.building
            && matches!(&self.found.lookup, Lookup::Current(build)
                if build.status.stage == perch_changescape::Stage::Facts)
    }

    /// The page is of an earlier head than the pull request's.
    pub fn stale(&self) -> bool {
        matches!(
            self.found.lookup,
            Lookup::Stale(_) | Lookup::Running(_, Some(_))
        )
    }
}

/// How many lines of a build's progress are kept.
pub const LOG_LINES: usize = 300;

thread_local! {
    /// Whether this Perch can build a changescape, as the backend last
    /// said, for the item menu, which has no open item to ask. Nothing
    /// offers a build until it says so.
    static CAN_BUILD: Cell<bool> = const { Cell::new(false) };
}

pub fn set_can_build(can: bool) {
    CAN_BUILD.with(|cell| cell.set(can));
}

/// Whether to offer a build: the skill installed, Claude Code found, and
/// Changescape on.
pub fn can_build() -> bool {
    CAN_BUILD.with(Cell::get)
}

/// Whether the open item's tab offers a build. Tracked.
fn tab_can_build(state: ReviewState) -> bool {
    shown(state).is_some_and(|shown| shown.found.can_build)
}

/// Build the item's changescape, and show the tab, where its progress
/// comes once the build starts. A build already running is the one shown.
pub fn request_build(app: AppState, backend: &Backend, item_id: &str) {
    if let Some(review) = app.review.get_untracked() {
        let open = review
            .item
            .with_untracked(|item| item.as_ref().is_some_and(|item| item.id == item_id));
        if open {
            if !shown(review).is_some_and(|shown| shown.lookup().is_running()) {
                review.changescape_log.set(Vec::new());
            }
            review.show_tab(crate::review::model::ItemTab::Changescape);
        }
    }
    backend.build_changescape(item_id);
}

/// The open item's changescape, when it has one to show. Tracked.
pub fn shown(state: ReviewState) -> Option<Arc<Shown>> {
    let shown = state.changescape.get()?;
    state
        .item
        .with(|item| item.as_ref().is_some_and(|item| item.id == shown.item_id))
        .then_some(shown)
}

/// Whether the switch offers Changescape. Tracked.
pub fn available(state: ReviewState) -> bool {
    shown(state).is_some_and(|shown| shown.has_tab())
}

/// Look the open pull request's changescape up again.
pub fn refresh(state: ReviewState, backend: &Backend) {
    let Some((item_id, repo)) = state.item.with_untracked(|item| {
        item.as_ref()
            .map(|item| (item.id.clone(), item.repo_full_name.clone()))
    }) else {
        return;
    };
    let Some((number, head)) = state.pull_request.with_untracked(|pull| {
        pull.as_ref()
            .map(|pull| (pull.number, pull.head_sha.clone()))
    }) else {
        return;
    };
    backend.lookup_changescape(&item_id, &repo, number, &head);
}

/// How often a running build's status is read again.
const POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// While a build of the open item runs, or this Perch's build is still
/// writing the narration of a page it drew, look it up again every
/// `POLL`. Each answer schedules the next look, so there is never more
/// than one waiting.
pub fn poll(state: ReviewState, backend: Rc<Backend>) {
    // Look the pull request up when the open item or its head changes.
    let opened = Memo::new(move |_| {
        let item = state
            .item
            .with(|item| item.as_ref().map(|item| item.id.clone()))?;
        let head = state
            .pull_request
            .with(|pull| pull.as_ref().map(|pull| pull.head_sha.clone()))?;
        Some((item, head))
    });
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            if opened.get().is_some() {
                refresh(state, &backend);
            }
        });
    }
    // The reader's marks come with what the backend found; a mark made
    // since is in the database by the time the next lookup reads it.
    floem::reactive::Effect::new(move |_| {
        if let Some(shown) = shown(state) {
            let (marks, changed) = stop::split_marks(shown.page(), &shown.found.marks);
            state.changescape_marks.set(marks);
            state.changescape_changed.set(changed);
        }
    });
    floem::reactive::Effect::new(move |_| {
        let Some(running) =
            shown(state).filter(|shown| shown.lookup().is_running() || shown.narration_pending())
        else {
            return;
        };
        let backend = backend.clone();
        floem::action::exec_after(POLL, move |_| {
            let still = state
                .changescape
                .with_untracked(|now| now.as_ref().is_some_and(|now| Arc::ptr_eq(now, &running)));
            if still {
                refresh(state, &backend);
            }
        });
    });
}

/// A value behind an `Arc`, equal to another only when it is the same
/// one: one read of a page, one load of a file.
pub(crate) struct SameArc<T>(pub Arc<T>);

impl<T> Clone for SameArc<T> {
    fn clone(&self) -> Self {
        SameArc(self.0.clone())
    }
}

impl<T> PartialEq for SameArc<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What the tab's body is.
#[derive(Clone, PartialEq)]
enum Body {
    Tour(SameArc<LoadedPage>),
    /// A build running with no earlier page to show meanwhile.
    Building,
    /// A failed build, and why.
    Failed(String),
    /// No changescape yet: the build, offered.
    Unbuilt,
    /// A page with no tour, or whose tour does not read: why, and whether
    /// to offer a rebuild.
    Refused {
        message: &'static str,
        rebuild: bool,
    },
    Nothing,
}

fn body_of(state: ReviewState) -> Body {
    let Some(shown) = shown(state) else {
        return Body::Nothing;
    };
    if let Some(page) = shown.page() {
        if page.tour().is_some() {
            return Body::Tour(SameArc(page.clone()));
        }
        // An earlier page with no tour is no use while a build runs: its
        // progress is.
        if !shown.lookup().is_running() {
            return Body::Refused {
                message: if page.tour_error().is_some() {
                    "This Changescape's tour could not be read."
                } else {
                    "This Changescape has no tour."
                },
                rebuild: shown.found.can_build,
            };
        }
    }
    match shown.lookup() {
        Lookup::Running(..) => Body::Building,
        Lookup::Failed(build) => Body::Failed(
            build
                .status
                .error
                .clone()
                .or_else(|| shown.found.error.clone())
                .unwrap_or_else(|| "The build wrote no page.".to_string()),
        ),
        _ => match &shown.found.error {
            Some(error) => Body::Failed(error.clone()),
            None if shown.found.can_build || shown.found.skill_too_old => Body::Unbuilt,
            None => Body::Nothing,
        },
    }
}

/// The tab's body: the tour, under the line that names its commit. While
/// a first build runs, its progress; after one fails, why.
pub fn view(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // Built again when the body changes, not on every write to the state.
    let body = Memo::new(move |_| body_of(state));
    dyn_container(
        move || body.get(),
        move |body| match body {
            Body::Building => building(state).into_any(),
            Body::Failed(error) => failed(app, state, backend.clone(), error).into_any(),
            Body::Unbuilt => unbuilt(app, state, backend.clone()).into_any(),
            Body::Refused { message, rebuild } => {
                refused(app, state, backend.clone(), message, rebuild).into_any()
            }
            Body::Nothing => Empty::new().into_any(),
            Body::Tour(SameArc(page)) => Stack::vertical((
                commit_bar(app, state, backend.clone(), page.clone()),
                tour::tour(app, state, backend.clone(), page)
                    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0)),
            ))
            .style(|s| {
                s.size_full()
                    .flex_col()
                    .position(floem::style::Position::Relative)
            })
            .into_any(),
        },
    )
    .debug_name("Changescape body")
    .style(|s| s.size_full().min_height(0.0))
}

/// A panel in place of the tour: a column at the width Conversation has.
fn panel(children: impl floem::view::IntoViewIter) -> Stack {
    Stack::vertical(children).style(|s| {
        s.flex_col()
            .items_start()
            .gap(4.0)
            .width(theme::CONVERSATION_WIDTH)
            .max_width_full()
            .padding(theme::CONVERSATION_PADDING)
    })
}

/// A panel's title.
fn panel_title(text: &'static str) -> Label {
    Label::new(text).style(|s| {
        s.font_size(theme::TEXT_SECTION_TITLE)
            .font_weight(theme::weight_strong())
            .color(theme::fg())
    })
}

/// A build's progress, while there is no page to show: the steps it has
/// taken, the last of them at the foot.
fn building(state: ReviewState) -> impl IntoView {
    let lines = move || {
        state.changescape_log.with(|log| {
            let from = log.len().saturating_sub(16);
            log[from..].join("\n")
        })
    };
    panel((
        panel_title("Building the changescape"),
        Label::new("Claude Code is running the Changescape skill. It takes a few minutes.")
            .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_muted())),
        Label::derived(lines).style(|s| {
            s.font_size(theme::PROSE_CODE_FONT_SIZE)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(theme::fg_muted())
                .line_height(1.6)
                .margin_top(12.0)
                .width_full()
        }),
    ))
    .debug_name("Changescape building")
}

/// A pull request with no changescape yet: what one is, and the button
/// that builds it.
fn unbuilt(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    panel((
        panel_title("No changescape yet"),
        Label::new(
            "A changescape explains how this pull request changes the shape of the code: \
             where it sits, what it moves, and what it does differently. Claude Code \
             builds it with the Changescape skill in a few minutes.",
        )
        .style(|s| {
            s.font_size(theme::TEXT_BODY)
                .color(theme::fg_muted())
                .line_height(theme::PROSE_LINE_HEIGHT)
                .width_full()
                .min_width(0.0)
        }),
        Container::new(Label::new("Build Changescape").style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::canvas())
                .selectable(false)
        }))
        .debug_name("Changescape build button")
        .on_event_stop(el::Click, move |_, _| build_open_item(app, state, &backend))
        .style(move |s| {
            crate::ui::widgets::clickable(
                s.margin_top(12.0)
                    .padding_horiz(14.0)
                    .height(32.0)
                    .items_center()
                    .border_radius(theme::RADIUS)
                    .background(theme::fg()),
            )
            .hover(|s| s.background(theme::fg_muted()))
            .apply_if(!tab_can_build(state), |s| s.hide())
        }),
        skill_note(state),
    ))
    .debug_name("Changescape unbuilt")
}

fn build_open_item(app: AppState, state: ReviewState, backend: &Backend) {
    let item = state
        .item
        .with_untracked(|item| item.as_ref().map(|item| item.id.clone()));
    if let Some(item) = item {
        request_build(app, backend, &item);
    }
}

/// A failed build: why, and a way to try again.
fn failed(app: AppState, state: ReviewState, backend: Rc<Backend>, error: String) -> impl IntoView {
    panel((
        panel_title("The changescape could not be built"),
        Label::new(error).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .width_full()
        }),
        Container::new(build_link(app, state, backend, "Retry")).style(|s| s.margin_top(8.0)),
        skill_note(state),
    ))
    .debug_name("Changescape failed")
}

/// A page this Perch does not draw: why, and Rebuild where a build can
/// run.
fn refused(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    message: &'static str,
    rebuild: bool,
) -> impl IntoView {
    let action = if rebuild {
        build_link(app, state, backend, "Rebuild").into_any()
    } else {
        Empty::new().into_any()
    };
    panel((
        Label::new(message).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .color(theme::fg())
                .width_full()
        }),
        Container::new(action).style(|s| s.margin_top(8.0)),
        skill_note(state),
    ))
    .debug_name("Changescape refused")
}

/// Why no build is offered, when the installed skill writes no tour.
fn skill_note(state: ReviewState) -> impl IntoView {
    Label::new(crate::backend::changescape::SKILL_TOO_OLD)
        .debug_name("Changescape skill too old")
        .style(move |s| {
            let too_old = shown(state).is_some_and(|shown| shown.found.skill_too_old);
            s.font_size(theme::TEXT_META)
                .color(theme::notice_fg())
                .margin_top(8.0)
                .width_full()
                .apply_if(!too_old, |s| s.hide())
        })
}

/// A link that builds the open item's changescape again. Absent when
/// this Perch cannot build one.
fn build_link(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    text: &'static str,
) -> impl IntoView {
    prose::link(text, move || build_open_item(app, state, &backend))
        .debug_name(format!("Changescape {}", text.to_lowercase()))
        .style(move |s| {
            let running = shown(state).is_some_and(|shown| shown.lookup().is_running());
            s.apply_if(!tab_can_build(state) || running, |s| s.hide())
        })
}

/// One line over the tour: the commit it is of, as a notice when that is
/// an earlier one, Rebuild, and how many read stops the reader marked
/// done.
fn commit_bar(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    page: Arc<LoadedPage>,
) -> impl IntoView {
    let text = move || {
        shown(state)
            .and_then(|shown| {
                let build = shown.lookup().drawable()?;
                let building = if shown.lookup().is_running() {
                    " Building one of the current head."
                } else if shown.narration_pending() {
                    " Writing the narration…"
                } else {
                    ""
                };
                Some(if shown.stale() {
                    format!(
                        "Changescape of an earlier commit, {}.{building}",
                        build.short_sha()
                    )
                } else {
                    format!("Changescape of {}.{building}", build.short_sha())
                })
            })
            .unwrap_or_default()
    };
    Stack::horizontal((
        Label::derived(text).style(move |s| {
            let stale = shown(state).is_some_and(|shown| shown.stale());
            s.font_size(theme::TEXT_META)
                .color(if stale {
                    theme::notice_fg()
                } else {
                    theme::fg_muted()
                })
                .selectable(false)
        }),
        build_link(app, state, backend, "Rebuild").style(|s| s.margin_left(8.0)),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        Label::derived(move || tour::progress_line(state, &page)).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .selectable(false)
        }),
    ))
    .debug_name("Changescape commit bar")
    .style(move |s| {
        let stale = shown(state).is_some_and(|shown| shown.stale());
        let s = s
            .width_full()
            .items_center()
            .padding_horiz(16.0)
            .padding_vert(6.0)
            .border_bottom(1.0);
        if stale {
            s.background(theme::notice_bg())
                .border_color(theme::notice_border())
        } else {
            s.background(theme::canvas())
                .border_color(theme::border_muted())
        }
    })
}

#[cfg(test)]
mod tests;
