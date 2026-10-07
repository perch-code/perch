//! The Changescape tab: Changescape's account of how a pull request changes
//! the shape of its codebase, drawn from the `page.json` it left in its
//! cache. A way into Perch, not a second code viewer: what it names opens
//! in Code, and its review threads open in Perch's own.
//!
//! With no changescape for the item, nothing here shows: no pill, no empty
//! state.

pub mod blocks;
pub mod navigate;
pub mod rail;
pub mod svg;

use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use perch_changescape::{LineIndex, Lookup, Page};

use crate::backend::changescape::Found;
use crate::backend::Backend;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;

/// What the backend found for one pull request at one head, with the
/// line index built once for it.
#[derive(Debug)]
pub struct Shown {
    pub item_id: String,
    pub head_sha: String,
    pub found: Arc<Found>,
    pub index: LineIndex,
}

impl Shown {
    pub fn new(item_id: String, head_sha: String, found: Arc<Found>) -> Shown {
        let index = found
            .page
            .as_deref()
            .map(LineIndex::new)
            .unwrap_or_default();
        Shown {
            item_id,
            head_sha,
            found,
            index,
        }
    }

    pub fn page(&self) -> Option<&Arc<Page>> {
        self.found.page.as_ref()
    }

    pub fn lookup(&self) -> &Lookup {
        &self.found.lookup
    }

    /// Whether the tab has anything to show: a page, or a build running.
    pub fn has_tab(&self) -> bool {
        self.found.page.is_some() || self.found.lookup.is_running()
    }

    /// The page is of an earlier head than the pull request's.
    pub fn stale(&self) -> bool {
        matches!(self.found.lookup, Lookup::Stale(_))
            || matches!(self.found.lookup, Lookup::Running(_, Some(_)))
    }
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

/// While a build of the open item runs, look it up again every two
/// seconds. Perch has no watcher on the cache; items that are not open
/// are not polled. Each answer schedules the next look, so there is never
/// more than one waiting.
pub fn poll(state: ReviewState, backend: Rc<Backend>) {
    floem::reactive::Effect::new(move |_| {
        let Some(running) = shown(state).filter(|shown| shown.lookup().is_running()) else {
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
pub fn selected_section(state: ReviewState, page: &Page) -> Option<String> {
    let picked = state.changescape_section.get();
    picked
        .filter(|id| page.sections.iter().any(|section| &section.id == id))
        .or_else(|| page.sections.first().map(|section| section.id.clone()))
}

/// A page, equal to another only when it is the same page: one read of
/// one build.
#[derive(Clone)]
struct SamePage(Arc<Page>);

impl PartialEq for SamePage {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// The tab's body: the rail of sections on the left, the selected section
/// on the right, and over both the banner of a stale page.
pub fn view(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let _ = app;
    // Built again when the page changes, not on every write to the state.
    let page =
        Memo::new(move |_| shown(state).and_then(|shown| shown.page().cloned().map(SamePage)));
    dyn_container(
        move || page.get(),
        move |page| match page.map(|page| page.0) {
            Some(page) => Stack::vertical((
                stale_banner(state),
                Stack::horizontal((
                    rail::rail(state, page.clone()),
                    section_body(state, backend.clone(), page.clone()),
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
            None => Empty::new().into_any(),
        },
    )
    .debug_name("Changescape body")
    .style(|s| s.size_full().min_height(0.0))
}

/// One line over a page drawn for an earlier commit.
fn stale_banner(state: ReviewState) -> impl IntoView {
    let text = move || {
        shown(state)
            .and_then(|shown| {
                let build = shown.lookup().drawable()?;
                shown
                    .stale()
                    .then(|| format!("Changescape of an earlier commit, {}.", build.short_sha()))
            })
            .unwrap_or_default()
    };
    Container::new(Label::derived(text).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::notice_fg())
            .selectable(false)
    }))
    .debug_name("Changescape stale banner")
    .style(move |s| {
        let stale = shown(state).is_some_and(|shown| shown.stale());
        s.width_full()
            .padding_horiz(16.0)
            .padding_vert(6.0)
            .background(theme::notice_bg())
            .border_bottom(1.0)
            .border_color(theme::notice_border())
            .apply_if(!stale, |s| s.hide())
    })
}

/// The excerpt of an element only on the base side, over the tab: its
/// own diff, and a way to its file in Code.
fn base_popover(state: ReviewState, backend: Rc<Backend>, page: Arc<Page>) -> impl IntoView {
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
            let rows = r.src.iter().map(|row| {
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
                            .font_size(theme::CODE_FONT_SIZE)
                            .color(theme::gutter_fg())
                            .width(theme::CODE_ROW_NUMBER_WIDTH)
                            .justify_end()
                            .padding_right(8.0)
                    }),
                    Label::new(format!("{} {}", row.op, row.text)).style(move |s| {
                        s.font_family(mono())
                            .font_size(theme::CODE_FONT_SIZE)
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

/// The selected section: each of its views, title, caption and blocks, one
/// under the other.
fn section_body(state: ReviewState, backend: Rc<Backend>, page: Arc<Page>) -> impl IntoView {
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
            let views: Vec<_> = page.section_views(section).into_iter().cloned().collect();
            let heading = Label::new(section.t.clone()).style(|s| {
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
            let cx = blocks::Ctx {
                state,
                backend: backend.clone(),
                page: page.clone(),
            };
            Stack::vertical((
                heading,
                question,
                Stack::vertical_from_iter(
                    views
                        .into_iter()
                        .map(move |view| blocks::view(cx.clone(), view).into_any()),
                )
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
    .style(|s| {
        s.flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_width(0.0)
            .height_full()
    })
}

/// Select a section on the tab, and show the tab.
pub fn show_section(state: ReviewState, id: &str) {
    state.changescape_section.set(Some(id.to_string()));
    state.show_tab(crate::review::model::ItemTab::Changescape);
}

/// Select the section that holds a view.
pub fn show_view(state: ReviewState, page: &Page, view: &str) {
    if let Some(section) = page.section_of_view(view) {
        show_section(state, &section.id);
    }
}

/// Write the section without a rebuild when it is already the one shown.
pub fn select(state: ReviewState, id: &str) {
    if state
        .changescape_section
        .with_untracked(|picked| picked.as_deref() != Some(id))
    {
        state
            .changescape_section
            .update(|picked| *picked = Some(id.to_string()));
    }
}

#[cfg(test)]
mod tests;
