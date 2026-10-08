//! The Changescape tab: Changescape's account of how a pull request changes
//! the shape of its codebase, drawn from the `page.json` it left in its
//! cache. A way into Perch, not a second code viewer: what it names opens
//! in Code, and its review threads open in Perch's own.
//!
//! With no changescape for the item, nothing here shows: no pill, no empty
//! state.

pub mod blocks;
pub mod navigate;
mod order;
pub mod rail;
pub mod strip;
pub mod svg;
pub mod threads;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use floem::AnyView;
use perch_changescape::v3::FacetOption;
use perch_changescape::{LineIndex, LoadedPage, Lookup};

use crate::backend::changescape::Found;
use crate::backend::Backend;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::menu::MenuExt;

/// What the backend found for one pull request at one head, with the
/// line index built once for it.
#[derive(Debug)]
pub struct Shown {
    pub item_id: String,
    pub found: Arc<Found>,
    pub index: LineIndex,
}

impl Shown {
    pub fn new(item_id: String, found: Arc<Found>) -> Shown {
        let index = found
            .page
            .as_deref()
            .map(|page| LineIndex::new(page))
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
    /// refuses or could not read, or a build this Perch can start.
    pub fn has_tab(&self) -> bool {
        self.found.page.is_some()
            || self.found.lookup.is_running()
            || self.found.lookup.refused().is_some()
            || self.unreadable()
            || self.found.can_build
    }

    /// A finished page that did not read: a bug in the page's producer,
    /// which the browser page may still draw.
    pub fn unreadable(&self) -> bool {
        self.found.page.is_none() && self.found.lookup.drawable().is_some()
    }

    /// The page is the current head's facts, and the build this Perch
    /// runs is still writing its captions.
    pub fn captions_pending(&self) -> bool {
        self.found.building
            && matches!(&self.found.lookup, Lookup::Current(build)
                if build.status.stage == perch_changescape::Stage::Facts)
    }

    /// The page is of an earlier head than the pull request's.
    pub fn stale(&self) -> bool {
        matches!(self.found.lookup, Lookup::Stale(_))
            || matches!(self.found.lookup, Lookup::Running(_, Some(_)))
    }
}

/// How many lines of a build's progress are kept.
pub const LOG_LINES: usize = 300;

thread_local! {
    /// Whether this Perch can build a changescape, as the backend last
    /// said. Nothing offers a build until it says so.
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
/// writing the captions of a page it drew, look it up again every
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
    floem::reactive::Effect::new(move |_| {
        let Some(running) =
            shown(state).filter(|shown| shown.lookup().is_running() || shown.captions_pending())
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

/// The selected section's id: the one the reader picked, if the page
/// still has it, else the first. Tracked.
pub fn selected_section(state: ReviewState, page: &LoadedPage) -> Option<String> {
    let picked = state.changescape_section.get();
    picked
        .filter(|id| page.sections.iter().any(|section| &section.id == id))
        .or_else(|| page.sections.first().map(|section| section.id.clone()))
}

/// Call `f` with the facet option the tab applies, if the page has it.
/// Tracked.
pub fn with_facet<R>(
    state: ReviewState,
    page: &LoadedPage,
    f: impl FnOnce(Option<&FacetOption>) -> R,
) -> R {
    state.changescape_facet.with(|picked| {
        f(picked
            .as_ref()
            .and_then(|(facet, option)| page.facet_option(facet, option)))
    })
}

/// The elements whose table rows never fold: the one whose base excerpt
/// is open, and the one the Code strip names.
fn pinned(state: ReviewState) -> Memo<Vec<String>> {
    Memo::new(move |_| {
        state
            .changescape_popover
            .get()
            .into_iter()
            .chain(strip::under_caret(state).map(|(el, _, _)| el))
            .collect()
    })
}

/// A page, equal to another only when it is the same page: one read of
/// one build.
#[derive(Clone)]
struct SamePage(Arc<LoadedPage>);

impl PartialEq for SamePage {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What the tab's body is.
#[derive(Clone, PartialEq)]
enum Body {
    Page(SamePage),
    /// A build running with no earlier page to show meanwhile.
    Building,
    /// A failed build, and why.
    Failed(String),
    /// No changescape yet: the build, offered.
    Unbuilt,
    /// A page an older Changescape wrote: a rebuild, offered.
    Outdated,
    /// A page that needs a newer Perch: the browser page, offered.
    TooNew,
    Nothing,
}

fn body_of(state: ReviewState) -> Body {
    let Some(shown) = shown(state) else {
        return Body::Nothing;
    };
    if let Some(page) = shown.page() {
        return Body::Page(SamePage(page.clone()));
    }
    match shown.lookup() {
        Lookup::Running(..) => Body::Building,
        Lookup::Outdated(_) => Body::Outdated,
        Lookup::TooNew(_) => Body::TooNew,
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
            None if shown.found.can_build => Body::Unbuilt,
            None => Body::Nothing,
        },
    }
}

/// The tab's body: the rail of sections on the left, the selected section
/// on the right, and over both the banner of a stale page. While a first
/// build runs, its progress; after one fails, why.
pub fn view(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // Built again when the body changes, not on every write to the state.
    let body = Memo::new(move |_| body_of(state));
    dyn_container(
        move || body.get(),
        move |body| match body {
            Body::Building => building(state).into_any(),
            Body::Failed(error) => failed(app, state, backend.clone(), error).into_any(),
            Body::Unbuilt => unbuilt(app, state, backend.clone()).into_any(),
            Body::Outdated => refused(
                app,
                state,
                backend.clone(),
                "This Changescape was built by an older version. Rebuild it to see this PR.",
                can_build(),
            )
            .into_any(),
            Body::TooNew => refused(
                app,
                state,
                backend.clone(),
                "This Changescape needs a newer Perch.",
                false,
            )
            .into_any(),
            Body::Nothing => Empty::new().into_any(),
            Body::Page(SamePage(page)) => Stack::vertical((
                commit_bar(app, state, backend.clone(), page.clone()),
                Stack::horizontal((
                    rail::rail(state, page.clone()),
                    section_side(state, backend.clone(), page.clone()),
                ))
                .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0)),
                base_popover(state, backend.clone(), page),
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

/// A build's progress, while there is no page to show: the steps it has
/// taken, the last of them at the foot.
fn building(state: ReviewState) -> impl IntoView {
    let lines = move || {
        state.changescape_log.with(|log| {
            let from = log.len().saturating_sub(16);
            log[from..].join("\n")
        })
    };
    Stack::vertical((
        Label::new("Building the changescape").style(|s| {
            s.font_size(theme::TEXT_SECTION_TITLE)
                .font_weight(theme::weight_strong())
                .color(theme::fg())
        }),
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
    .style(|s| {
        s.flex_col()
            .gap(4.0)
            .width(theme::CONVERSATION_WIDTH)
            .max_width_full()
            .padding(theme::CONVERSATION_PADDING)
    })
}

/// A pull request with no changescape yet: what one is, and the button
/// that builds it.
fn unbuilt(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Stack::vertical((
        Label::new("No changescape yet").style(|s| {
            s.font_size(theme::TEXT_SECTION_TITLE)
                .font_weight(theme::weight_strong())
                .color(theme::fg())
        }),
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
        .style(|s| {
            crate::ui::widgets::clickable(
                s.margin_top(12.0)
                    .padding_horiz(14.0)
                    .height(32.0)
                    .items_center()
                    .border_radius(theme::RADIUS)
                    .background(theme::fg()),
            )
            .hover(|s| s.background(theme::fg_muted()))
        }),
    ))
    .debug_name("Changescape unbuilt")
    .style(|s| {
        s.flex_col()
            .items_start()
            .gap(4.0)
            .width(theme::CONVERSATION_WIDTH)
            .max_width_full()
            .padding(theme::CONVERSATION_PADDING)
    })
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
    Stack::vertical((
        Label::new("The changescape could not be built").style(|s| {
            s.font_size(theme::TEXT_SECTION_TITLE)
                .font_weight(theme::weight_strong())
                .color(theme::fg())
        }),
        Label::new(error).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .width_full()
        }),
        Stack::horizontal((
            build_link(app, state, backend, "Retry"),
            blocks::browser_link(state, "Open the browser page").style(move |s| {
                let unreadable = shown(state).is_some_and(|shown| shown.unreadable());
                s.margin_left(12.0).apply_if(!unreadable, |s| s.hide())
            }),
        ))
        .style(|s| s.margin_top(8.0)),
    ))
    .debug_name("Changescape failed")
    .style(|s| {
        s.flex_col()
            .gap(4.0)
            .width(theme::CONVERSATION_WIDTH)
            .max_width_full()
            .padding(theme::CONVERSATION_PADDING)
    })
}

/// A page this Perch does not draw: why, and Rebuild where a build can
/// run, else the browser page, which draws every version.
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
        blocks::browser_link(state, "Open the browser page").into_any()
    };
    Stack::vertical((
        Label::new(message).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .color(theme::fg())
                .width_full()
        }),
        Container::new(action).style(|s| s.margin_top(8.0)),
    ))
    .debug_name("Changescape refused")
    .style(|s| {
        s.flex_col()
            .gap(4.0)
            .width(theme::CONVERSATION_WIDTH)
            .max_width_full()
            .padding(theme::CONVERSATION_PADDING)
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
    Container::new(Label::new(text).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::link())
            .selectable(false)
    }))
    .debug_name(format!("Changescape {}", text.to_lowercase()))
    .on_event_stop(el::Click, move |_, _| build_open_item(app, state, &backend))
    .style(move |s| {
        let running = shown(state).is_some_and(|shown| shown.lookup().is_running());
        crate::ui::widgets::clickable(crate::ui::widgets::underlined_link(s))
            .apply_if(!can_build() || running, |s| s.hide())
    })
}

/// The picker's menu: the whole change, then each facet's options under
/// the facet's label.
pub(crate) fn facet_menu(state: ReviewState, page: &LoadedPage) -> crate::ui::menu::Menu {
    let picked = state.changescape_facet.get_untracked();
    let mut menu = crate::ui::menu::Menu::new().item("Whole change", |m| {
        m.checked(picked.is_none())
            .action(move || state.changescape_facet.set(None))
    });
    for facet in &page.facets {
        let options: Vec<(String, String, bool)> = facet
            .options
            .iter()
            .map(|option| {
                let on = picked.as_ref() == Some(&(facet.id.clone(), option.id.clone()));
                (option.id.clone(), option.label.clone(), on)
            })
            .collect();
        let facet_id = facet.id.clone();
        menu = menu.submenu(facet.label.clone(), move |mut sub| {
            for (id, label, on) in options {
                let pick = (facet_id.clone(), id);
                sub = sub.item(label, move |m| {
                    m.checked(on)
                        .action(move || state.changescape_facet.set(Some(pick.clone())))
                });
            }
            sub
        });
    }
    menu
}

/// The facet picker, and the toggle that hides table rows outside the
/// option instead of dimming them. Nothing for a page with no facets.
fn facet_picker(state: ReviewState, page: Arc<LoadedPage>) -> AnyView {
    if page.facets.is_empty() {
        return Empty::new().into_any();
    }
    let label = {
        let page = page.clone();
        move || {
            with_facet(state, &page, |option| {
                let name = option.map_or("Whole change", |option| option.label.as_str());
                format!("{name} ▾")
            })
        }
    };
    let active = {
        let page = page.clone();
        move || with_facet(state, &page, |option| option.is_some())
    };
    let picker = Container::new(Label::derived(label).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::fg())
            .selectable(false)
            .text_ellipsis()
            .min_width(0.0)
    }))
    .debug_name("Changescape facet picker")
    .dropdown(move || facet_menu(state, &page))
    .style(|s| {
        crate::ui::widgets::clickable(
            s.padding_horiz(8.0)
                .height(24.0)
                .max_width(320.0)
                .items_center()
                .border(1.0)
                .border_color(theme::border_muted())
                .border_radius(theme::RADIUS),
        )
        .hover(|s| s.background(theme::canvas_hover()))
    });
    let hide = state.changescape_facet_hide;
    let toggle = Container::new(
        Label::derived(move || format!("{} Hide the rest", if hide.get() { "☑" } else { "☐" }))
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .selectable(false)
            }),
    )
    .debug_name("Changescape facet hide")
    .on_event_stop(el::Click, move |_, _| hide.update(|on| *on = !*on))
    .style(move |s| {
        crate::ui::widgets::clickable(s.margin_left(10.0)).apply_if(!active(), |s| s.hide())
    });
    Stack::horizontal((picker, toggle))
        .style(|s| s.items_center().margin_left(16.0).min_width(0.0))
        .into_any()
}

/// One line over every page: the commit it is of, as a notice when that
/// is an earlier one, Rebuild, and the facet picker.
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
                } else if shown.captions_pending() {
                    " Writing the captions…"
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
        facet_picker(state, page),
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

/// The excerpt of an element only on the base side, over the tab: its
/// own diff, and a way to its file in Code.
fn base_popover(state: ReviewState, backend: Rc<Backend>, page: Arc<LoadedPage>) -> impl IntoView {
    dyn_container(
        move || state.changescape_popover.get(),
        move |el| {
            let Some((element, r)) = el.as_ref().and_then(|el| {
                let element = page.elements.get(el)?;
                Some((element.clone(), navigate::base_ref(element)?.clone()))
            }) else {
                return Empty::new().into_any();
            };
            let mono = || theme::CODE_FONT_STACK.to_string();
            let rows = perch_changescape::page::src_rows(&r)
                .into_iter()
                .map(|row| {
                    let number = row
                        .base
                        .or(row.head)
                        .map(|n| n.to_string())
                        .unwrap_or_default();
                    let ink = match row.op.as_str() {
                        "-" => theme::diff_deleted_fg(),
                        _ => theme::fg(),
                    };
                    let fill = match row.op.as_str() {
                        "-" => Some(theme::diff_deleted_bg()),
                        "+" => Some(theme::diff_added_bg()),
                        _ => None,
                    };
                    Stack::horizontal((
                        Label::new(number).style(move |s| {
                            s.font_family(mono())
                                .font_size(theme::code_font_size())
                                .color(theme::gutter_fg())
                                .width(theme::CODE_ROW_NUMBER_WIDTH)
                                .justify_end()
                                .padding_right(8.0)
                        }),
                        Label::new(format!("{} {}", row.op, row.text)).style(move |s| {
                            s.font_family(mono())
                                .font_size(theme::code_font_size())
                                .color(ink)
                        }),
                    ))
                    .style(move |s| {
                        s.height(theme::CODE_ROW_HEIGHT)
                            .items_center()
                            .width_full()
                            .apply_opt(fill, |s, fill| s.background(fill))
                    })
                    .into_any()
                });
            let show_backend = backend.clone();
            let show_ref = r.clone();
            Stack::vertical((
                Stack::horizontal((
                    Label::new(element.t.clone()).style(|s| {
                        s.font_size(theme::TEXT_BODY)
                            .font_weight(theme::weight_emphasis())
                            .color(theme::fg())
                    }),
                    Label::new(format!("{} · {}", element.k, r.f)).style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::fg_muted())
                            .margin_left(8.0)
                            .flex_grow(1.0_f32)
                            .min_width(0.0)
                            .text_ellipsis()
                    }),
                    Container::new(Label::new("Show in diff"))
                        .debug_name("Changescape show in diff")
                        .on_event_stop(el::Click, move |_, _| {
                            navigate::show_in_diff(state, &show_backend, &show_ref)
                        })
                        .style(|s| {
                            crate::ui::widgets::clickable(s)
                                .font_size(theme::TEXT_META)
                                .color(theme::link())
                                .margin_left(12.0)
                        }),
                    Container::new(Label::new("Close"))
                        .on_event_stop(el::Click, move |_, _| state.changescape_popover.set(None))
                        .style(|s| {
                            crate::ui::widgets::clickable(s)
                                .font_size(theme::TEXT_META)
                                .color(theme::fg_muted())
                                .margin_left(12.0)
                        }),
                ))
                .style(|s| s.items_center().width_full().padding_bottom(8.0)),
                Stack::vertical_from_iter(rows)
                    .style(|s| s.flex_col().width_full())
                    .scroll()
                    .style(|s| s.max_height(320.0).width_full()),
            ))
            .debug_name("Changescape base popover")
            .style(|s| {
                s.flex_col()
                    .width(640.0)
                    .max_width_pct(90.0)
                    .padding(theme::CARD_PADDING)
                    .background(theme::menu_bg())
                    .border(1.0)
                    .border_color(theme::menu_ring())
                    .border_radius(theme::CARD_RADIUS)
                    .box_shadow_blur(16.0)
                    .box_shadow_color(theme::menu_shadow())
            })
            .into_any()
        },
    )
    .style(move |s| {
        s.position(floem::style::Position::Absolute)
            .inset_top(48.0)
            .inset_left(240.0)
            .z_index(2)
            .apply_if(state.changescape_popover.with(Option::is_none), |s| {
                s.hide()
            })
    })
}

/// Right of the rail: the selected section, and over it an expanded
/// diagram.
fn section_side(state: ReviewState, backend: Rc<Backend>, page: Arc<LoadedPage>) -> impl IntoView {
    let cx = blocks::Ctx {
        state,
        backend,
        page,
        counts: threads::counts(state),
        pinned: pinned(state),
    };
    Stack::vertical((section_body(cx.clone()), expanded(cx))).style(|s| {
        s.flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_width(0.0)
            .height_full()
            .position(floem::style::Position::Relative)
    })
}

/// The diagram Expand opened, over the whole section and fitted to it,
/// with its own zoom and scroll. The rail and the commit bar stay.
fn expanded(cx: blocks::Ctx) -> impl IntoView {
    let state = cx.state;
    dyn_container(
        move || state.changescape_expanded.get(),
        move |at| {
            let Some((view, i)) = at else {
                return Empty::new().into_any();
            };
            let Some(block) = cx.page.blocks(&view).get(i) else {
                return Empty::new().into_any();
            };
            let title = block
                .title()
                .map(str::to_string)
                .or_else(|| cx.page.view(&view).map(|view| view.t.clone()))
                .unwrap_or_default();
            Stack::vertical((
                Label::new(title).style(|s| {
                    s.font_size(theme::TEXT_SECTION_TITLE)
                        .font_weight(theme::weight_strong())
                        .color(theme::fg())
                }),
                Container::new(svg::svg_block(
                    cx.clone(),
                    block.svg().unwrap_or_default(),
                    svg::Frame::Expanded,
                ))
                .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0)),
            ))
            .debug_name("Changescape expanded diagram")
            .style(|s| {
                s.flex_col()
                    .gap(8.0)
                    .size_full()
                    .padding(16.0)
                    .background(theme::canvas())
            })
            .into_any()
        },
    )
    .style(move |s| {
        s.position(floem::style::Position::Absolute)
            .inset(0.0)
            .z_index(1)
            .apply_if(state.changescape_expanded.with(Option::is_none), |s| {
                s.hide()
            })
    })
}

/// The selected section: each of its views, title, caption and blocks, one
/// under the other.
fn section_body(cx: blocks::Ctx) -> impl IntoView {
    let (state, page) = (cx.state, cx.page.clone());
    let selected = {
        let page = page.clone();
        Memo::new(move |_| selected_section(state, &page))
    };
    dyn_container(
        move || selected.get(),
        move |id| {
            let Some(section) = id
                .as_ref()
                .and_then(|id| page.sections.iter().find(|section| &section.id == id))
            else {
                return Empty::new().into_any();
            };
            let section_title = section.label.clone();
            let views: Vec<_> = page.section_views(section).into_iter().cloned().collect();
            let heading = Label::new(section.label.clone()).style(|s| {
                s.font_size(theme::TEXT_PAGE_TITLE)
                    .font_weight(theme::weight_strong())
                    .color(theme::fg())
                    .line_height(theme::PAGE_TITLE_LINE_HEIGHT)
            });
            let question = Label::new(section.q.clone()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::fg_muted())
                    .margin_top(4.0)
            });
            let cx = cx.clone();
            Stack::vertical((
                heading,
                question,
                facet_note(state, page.clone()),
                Stack::vertical_from_iter(views.into_iter().map(move |view| {
                    let titled = view.t != section_title;
                    let shown = cx.page.blocks(&view.id).to_vec();
                    blocks::view(cx.clone(), view, shown, titled).into_any()
                }))
                .style(|s| s.flex_col().gap(32.0).margin_top(24.0).width_full()),
            ))
            .debug_name(format!("Changescape section {}", section.id))
            .style(|s| {
                s.flex_col()
                    .width(theme::CONVERSATION_WIDTH)
                    .max_width_full()
            })
            .into_any()
        },
    )
    .style(|s| {
        s.width_full()
            .items_start()
            .justify_center()
            .padding(theme::CONVERSATION_PADDING)
    })
    .scroll()
    .style(|s| s.size_full())
}

/// While a facet option is on: its note, muted, and the way back to the
/// whole change.
fn facet_note(state: ReviewState, page: Arc<LoadedPage>) -> impl IntoView {
    let shown = {
        let page = page.clone();
        Memo::new(move |_| {
            with_facet(state, &page, |option| {
                option.map(|option| option.note.clone().unwrap_or_else(|| option.label.clone()))
            })
        })
    };
    dyn_container(
        move || shown.get(),
        move |note| {
            let Some(note) = note else {
                return Empty::new().into_any();
            };
            Stack::horizontal((
                Label::new(note).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_muted())
                        .min_width(0.0)
                        .flex_shrink(1.0_f32)
                }),
                Container::new(Label::new("Whole change").style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::link())
                        .selectable(false)
                }))
                .debug_name("Changescape whole change")
                .on_event_stop(el::Click, move |_, _| state.changescape_facet.set(None))
                .style(|s| {
                    crate::ui::widgets::clickable(crate::ui::widgets::underlined_link(s))
                        .margin_left(10.0)
                        .flex_shrink(0.0_f32)
                }),
            ))
            .debug_name("Changescape facet note")
            .style(|s| s.items_baseline().width_full().margin_top(12.0))
            .into_any()
        },
    )
    .style(|s| s.width_full())
}

/// Select a section on the tab, and show the tab.
pub fn show_section(state: ReviewState, id: &str) {
    state.changescape_expanded.set(None);
    state.changescape_section.set(Some(id.to_string()));
    state.show_tab(crate::review::model::ItemTab::Changescape);
}

/// Select the section that holds a view.
pub fn show_view(state: ReviewState, page: &LoadedPage, view: &str) {
    if let Some(section) = page.section_of_view(view) {
        show_section(state, &section.id);
    }
}

/// Select a section, writing nothing when it is already selected.
pub fn select(state: ReviewState, id: &str) {
    if state
        .changescape_section
        .with_untracked(|picked| picked.as_deref() != Some(id))
    {
        state.changescape_expanded.set(None);
        state
            .changescape_section
            .update(|picked| *picked = Some(id.to_string()));
    }
}

#[cfg(test)]
mod tests;
