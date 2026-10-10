//! The tour's two sides. On the left, the outline: the start, each
//! chapter with its stops and their marks, and the end. On the right, the
//! code map: the changed files by package, each with its added and
//! removed lines. Both light up the stop in view, the last one whose top
//! passed a third of the column, and a click on either goes there. A file
//! in no stop is in italics and opens in Code.
//!
//! A narrow tab drops the code map first, then the outline.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Container, Decorators, Empty, Label, Stack};
use floem::{AnyView, ViewId};
use perch_changescape::page::{self, Verdict};
use perch_changescape::v3::TourStop;
use perch_github::TourMark;

use crate::review::changescape::navigate;
use crate::review::changescape::prose::{self, hover_row, Ink};
use crate::review::changescape::stop::MarkShown;
use crate::review::changescape::tour::{go_to, toggle, Ctx};
use crate::review::model::ReviewState;
use crate::theme;
use crate::ui::nav_row::thousands;

const OUTLINE_WIDTH: f64 = 210.0;
const MAP_WIDTH: f64 = 230.0;
/// The tab's width under which the code map goes, and then the outline.
const MAP_FROM: f64 = 900.0;
const OUTLINE_FROM: f64 = 640.0;
/// A code map this small opens every package.
const MAP_OPEN_FILES: usize = 24;
const MAP_OPEN_PACKAGES: usize = 3;
/// The widest bar, for the file with the most changed lines.
const BAR_MAX: f64 = 40.0;

/// The id the cover takes in the tour's focus.
pub const COVER: &str = "cover";
/// The id the ending takes in the tour's focus.
pub const ENDING: &str = "ending";

/// Where each stop is, and the ending, in the order they come, to find
/// the stop in view.
#[derive(Clone, Default)]
pub struct Placed(Rc<RefCell<Vec<Spot>>>);

/// A stop, or the ending with `None`, and its view.
type Spot = (Option<String>, ViewId);

/// A file of the code map: its path, and its added and removed lines.
type MapFile = (String, i64, i64);

impl Placed {
    /// Note where a stop is, or the ending with `None`.
    pub fn add(&self, stop: Option<String>, view: ViewId) {
        self.0.borrow_mut().push((stop, view));
    }

    /// The stop whose top is the last above `line`, a y in the scrolled
    /// content of `column`. None above the first stop and past the
    /// ending's top.
    fn at(&self, column: ViewId, line: f64) -> Option<String> {
        let mut here = None;
        for (stop, view) in self.0.borrow().iter() {
            if content_y(*view, column).is_none_or(|y| y >= line) {
                break;
            }
            here = stop.clone();
        }
        here
    }
}

/// Where a view's top is in the content of the scroll `column`, from the
/// layout alone: the scroll moves the content, not its layout.
fn content_y(view: ViewId, column: ViewId) -> Option<f64> {
    let (mut y, mut at) = (0.0, view);
    loop {
        let parent = at.parent()?;
        if parent == column {
            return Some(y);
        }
        y += at.get_layout_rect().y0;
        at = parent;
    }
}

/// Find the stop in view again after the column scrolled to `offset`.
pub fn track(state: ReviewState, placed: &Placed, column: ViewId, offset: f64) {
    let height = column.get_layout_rect().height();
    let here = placed.at(column, offset + height / 3.0);
    if state.changescape_here.with_untracked(|now| *now != here) {
        state.changescape_here.set(here);
    }
}

/// The column between the sides, and each side where the tab has room.
pub fn sides(cx: &Ctx, column: AnyView) -> impl IntoView {
    let width = RwSignal::new(f64::MAX);
    let map = !cx.tour().map.is_empty();
    Stack::horizontal((
        outline(cx).style(move |s| s.apply_if(width.get() < OUTLINE_FROM, |s| s.hide())),
        Container::new(column).style(|s| {
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .height_full()
        }),
        code_map(cx).style(move |s| s.apply_if(!map || width.get() < MAP_FROM, |s| s.hide())),
    ))
    .on_event_cont(
        floem::context::LayoutChanged::listener(),
        move |_, floem::context::LayoutChanged { new_box, .. }| {
            let now = new_box.width();
            if width.get_untracked() != now {
                width.set(now);
            }
        },
    )
    .style(|s| s.size_full().min_height(0.0))
}

fn small(color: fn() -> floem::peniko::Color) -> Ink {
    Ink {
        size: theme::TEXT_META,
        weight: theme::weight_body,
        color,
    }
}

fn side_label(text: String) -> impl IntoView {
    Label::new(text).style(|s| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::fg_subtle())
            .selectable(false)
    })
}

// ---------------------------------------------------------------------
// The outline
// ---------------------------------------------------------------------

fn outline(cx: &Ctx) -> impl IntoView {
    let tour = cx.tour();
    let state = cx.state;
    let many = tour.chapters.len() > 1;
    let mut rows: Vec<AnyView> = vec![heading_row(
        state,
        COVER.to_string(),
        "Start".to_string(),
        cx.page.meta.title.clone(),
    )];
    let mut n = 0;
    for (i, chapter) in tour.chapters.iter().enumerate() {
        let label = if many {
            format!("Part {}", i + 1)
        } else {
            "The change".to_string()
        };
        rows.push(heading_row(
            state,
            chapter.id.clone(),
            label,
            chapter.title.clone(),
        ));
        let sections = page::section_of(chapter);
        let mut labeled = HashSet::new();
        for stop in &chapter.stops {
            if let Some(&k) = sections.get(stop.id.as_str()) {
                if labeled.insert(k) {
                    rows.push(section_label(&chapter.sections[k].title));
                }
            }
            n += 1;
            let skim = page::verdict(chapter, stop) == Verdict::Skim;
            rows.push(stop_row(state, n, stop, skim));
        }
    }
    rows.push(heading_row(
        state,
        ENDING.to_string(),
        "End".to_string(),
        "Where you stand".to_string(),
    ));
    Stack::vertical_from_iter(rows)
        .style(|s| s.flex_col().gap(1.0).width_full().padding(10.0))
        .scroll()
        .debug_name("Changescape outline")
        .style(|s| {
            s.width(OUTLINE_WIDTH)
                .flex_shrink(0.0_f32)
                .height_full()
                .background(theme::rail_bg())
                .border_right(1.0)
                .border_color(theme::border_muted())
        })
}

/// The start, a chapter, or the end: a small label over its title.
fn heading_row(state: ReviewState, id: String, label: String, title: String) -> AnyView {
    let go = id.clone();
    Container::new(
        Stack::vertical((
            side_label(label),
            prose::line(
                &title,
                Ink {
                    weight: theme::weight_emphasis,
                    ..small(theme::fg)
                },
            ),
        ))
        .style(|s| s.flex_col().width_full().min_width(0.0)),
    )
    .debug_name(format!("Changescape outline {id}"))
    .on_event_stop(el::Click, move |_, _| go_to(state, &go))
    .style(|s| hover_row(s.margin_top(8.0).padding_vert(4.0).padding_horiz(6.0)))
    .into_any()
}

/// A section's title, a muted label over its first stop.
fn section_label(title: &str) -> AnyView {
    Container::new(prose::line(title, small(theme::fg_subtle)))
        .debug_name("Changescape outline section")
        .style(|s| {
            s.width_full()
                .margin_top(6.0)
                .padding_horiz(6.0)
                .padding_vert(2.0)
        })
        .into_any()
}

/// A stop: its mark or its number, its title, and skim for a skim stop.
fn stop_row(state: ReviewState, n: usize, stop: &TourStop, skim: bool) -> AnyView {
    let id = stop.id.clone();
    let mark = {
        let id = id.clone();
        Memo::new(move |_| MarkShown::of(state, &id))
    };
    let here = {
        let id = id.clone();
        Memo::new(move |_| {
            state
                .changescape_here
                .with(|h| h.as_deref() == Some(id.as_str()))
        })
    };
    let go = id.clone();
    Container::new(
        Stack::horizontal((
            Label::derived(move || mark.get().glyph(&n.to_string())).style(|s| {
                s.width(22.0)
                    .flex_shrink(0.0_f32)
                    .font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .selectable(false)
            }),
            prose::line(
                &stop.title,
                small(if skim { theme::fg_muted } else { theme::fg }),
            ),
            Label::new("skim").style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .margin_left(6.0)
                    .flex_shrink(0.0_f32)
                    .selectable(false)
                    .apply_if(!skim, |s| s.hide())
            }),
        ))
        .style(|s| s.items_center().width_full().min_width(0.0)),
    )
    .debug_name(format!("Changescape outline stop {id}"))
    .on_event_stop(el::Click, move |_, _| go_to(state, &go))
    .style(move |s| {
        hover_row(
            s.padding_vert(3.0)
                .padding_horiz(6.0)
                .apply_if(here.get(), |s| s.background(theme::accent_subtle())),
        )
    })
    .into_any()
}

// ---------------------------------------------------------------------
// The code map
// ---------------------------------------------------------------------

/// The files of each stop, from its line counts and its pictures.
fn stop_paths(cx: &Ctx) -> Vec<(String, HashSet<String>)> {
    page::stops(cx.tour())
        .map(|(_, stop)| {
            let paths = stop
                .files
                .iter()
                .map(|f| f.path.clone())
                .chain(stop.pictures.iter().filter_map(|p| p.file.clone()))
                .collect();
            (stop.id.clone(), paths)
        })
        .collect()
}

fn code_map(cx: &Ctx) -> impl IntoView {
    let tour = cx.tour();
    let stops = Rc::new(stop_paths(cx));
    let sizes: HashMap<&str, (i64, i64)> = cx
        .page
        .files
        .iter()
        .map(|f| (f.path.as_str(), (f.add, f.del)))
        .collect();
    let packages: Vec<(String, Vec<MapFile>)> = tour
        .map
        .iter()
        .map(|p| {
            let files = p
                .files
                .iter()
                .filter_map(|path| {
                    let (add, del) = sizes.get(path.as_str())?;
                    Some((path.clone(), *add, *del))
                })
                .collect::<Vec<_>>();
            (p.pkg.clone(), files)
        })
        .filter(|(_, files)| !files.is_empty())
        .collect();
    let all: usize = packages.iter().map(|(_, files)| files.len()).sum();
    let most = packages
        .iter()
        .flat_map(|(_, files)| files.iter().map(|(_, add, del)| add + del))
        .max()
        .unwrap_or(1)
        .max(1);
    let open_all = packages.len() <= MAP_OPEN_PACKAGES || all <= MAP_OPEN_FILES;
    let opened = RwSignal::new(HashSet::<String>::new());
    let mut rows: Vec<AnyView> = vec![Stack::horizontal((
        Label::new("Code map").style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_muted())
                .selectable(false)
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        side_label(format!("{} files", thousands(all as i64))),
    ))
    .style(|s| s.items_center().width_full().padding_horiz(6.0))
    .into_any()];
    rows.push(
        Label::new("The files of the stop in view light up. A check means their stops are done. A file in italics is in no stop.")
            .style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .width_full()
                    .padding_horiz(6.0)
                    .margin_bottom(6.0)
            })
            .into_any(),
    );
    for (pkg, files) in packages {
        rows.push(package(cx, &stops, pkg, files, most, open_all, opened));
    }
    Stack::vertical_from_iter(rows)
        .style(|s| s.flex_col().gap(1.0).width_full().padding(10.0))
        .scroll()
        .debug_name("Changescape code map")
        .style(|s| {
            s.width(MAP_WIDTH)
                .flex_shrink(0.0_f32)
                .height_full()
                .background(theme::rail_bg())
                .border_left(1.0)
                .border_color(theme::border_muted())
        })
}

/// Whether a stop in view holds one of `paths`. Tracked.
fn in_view(state: ReviewState, stops: &[(String, HashSet<String>)], paths: &[String]) -> bool {
    state.changescape_here.with(|here| {
        here.as_deref().is_some_and(|here| {
            stops
                .iter()
                .find(|(id, _)| id == here)
                .is_some_and(|(_, held)| paths.iter().any(|p| held.contains(p)))
        })
    })
}

fn package(
    cx: &Ctx,
    stops: &Rc<Vec<(String, HashSet<String>)>>,
    pkg: String,
    files: Vec<MapFile>,
    most: i64,
    open_all: bool,
    opened: RwSignal<HashSet<String>>,
) -> AnyView {
    let state = cx.state;
    let paths: Vec<String> = files.iter().map(|(path, ..)| path.clone()).collect();
    let here = {
        let stops = stops.clone();
        Memo::new(move |_| in_view(state, &stops, &paths))
    };
    let open = {
        let pkg = pkg.clone();
        Memo::new(move |_| open_all || here.get() || opened.with(|o| o.contains(&pkg)))
    };
    let pkg_id = pkg.clone();
    let header = Container::new(
        Stack::horizontal((
            Label::derived(move || if open.get() { "▾" } else { "▸" }.to_string()).style(|s| {
                s.width(14.0)
                    .font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .selectable(false)
            }),
            Label::new(pkg.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(theme::fg())
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_shrink(1.0_f32)
                    .selectable(false)
            }),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            side_label(files.len().to_string()),
        ))
        .style(|s| s.items_center().width_full().min_width(0.0)),
    )
    .debug_name(format!("Changescape map package {pkg}"))
    .on_event_stop(el::Click, move |_, _| toggle(opened, &pkg_id))
    .style(move |s| {
        hover_row(
            s.margin_top(4.0)
                .padding_vert(3.0)
                .padding_horiz(6.0)
                .apply_if(here.get(), |s| s.background(theme::accent_subtle())),
        )
    });
    let rows: Vec<AnyView> = files
        .into_iter()
        .map(|(path, add, del)| file_row(cx, stops, &pkg, path, add, del, most))
        .collect();
    Stack::vertical((
        header,
        Stack::vertical_from_iter(rows).style(move |s| {
            s.flex_col()
                .width_full()
                .apply_if(!open.get(), |s| s.hide())
        }),
    ))
    .style(|s| s.flex_col().width_full())
    .into_any()
}

/// The path to show under its package: without the package's folder
/// where the path starts with it.
fn short_path<'a>(pkg: &str, path: &'a str) -> &'a str {
    path.strip_prefix(pkg)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(path)
}

fn file_row(
    cx: &Ctx,
    stops: &Rc<Vec<(String, HashSet<String>)>>,
    pkg: &str,
    path: String,
    add: i64,
    del: i64,
    most: i64,
) -> AnyView {
    let state = cx.state;
    let held: Vec<String> = stops
        .iter()
        .filter(|(_, paths)| paths.contains(&path))
        .map(|(id, _)| id.clone())
        .collect();
    let in_stop = !held.is_empty();
    let here = {
        let (stops, path) = (stops.clone(), path.clone());
        Memo::new(move |_| in_view(state, &stops, std::slice::from_ref(&path)))
    };
    let done = {
        let held = held.clone();
        Memo::new(move |_| {
            !held.is_empty()
                && state
                    .changescape_marks
                    .with(|m| held.iter().all(|id| m.get(id) == Some(&TourMark::Done)))
        })
    };
    let bar = |n: i64, color: fn() -> floem::peniko::Color| {
        let width = if n == 0 {
            0.0
        } else {
            (BAR_MAX * n as f64 / most as f64).max(2.0)
        };
        Empty::new().style(move |s| s.width(width).height(4.0).background(color()))
    };
    let name = short_path(pkg, &path).to_string();
    let open = {
        let (cx, path) = (cx.clone(), path.clone());
        move || match held.first() {
            Some(stop) => go_to(cx.state, stop),
            None => navigate::open_file_line(cx.state, &cx.backend, &path, None),
        }
    };
    Container::new(
        Stack::horizontal((
            Label::derived(move || if done.get() { "✓" } else { "" }.to_string()).style(|s| {
                s.width(14.0)
                    .flex_shrink(0.0_f32)
                    .font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .selectable(false)
            }),
            Label::new(name).style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(if in_stop {
                        theme::fg_muted()
                    } else {
                        theme::fg_subtle()
                    })
                    .apply_if(!in_stop, |s| s.font_style(floem::text::FontStyle::Italic))
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_shrink(1.0_f32)
                    .selectable(false)
            }),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            Stack::horizontal((
                bar(add, theme::diff_added_bar),
                bar(del, theme::diff_deleted_bar),
            ))
            .style(|s| {
                s.items_center()
                    .gap(1.0)
                    .margin_left(6.0)
                    .flex_shrink(0.0_f32)
            }),
        ))
        .style(|s| s.items_center().width_full().min_width(0.0)),
    )
    .debug_name(format!("Changescape map file {path}"))
    .on_event_stop(el::Click, move |_, _| open())
    .style(move |s| {
        hover_row(
            s.padding_vert(2.0)
                .padding_left(20.0)
                .padding_right(6.0)
                .apply_if(here.get(), |s| s.background(theme::accent_subtle())),
        )
    })
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_shows_without_its_package_folder() {
        assert_eq!(short_path("crates/a", "crates/a/src/lib.rs"), "src/lib.rs");
        assert_eq!(
            short_path("perch_app", "crates/a/src/lib.rs"),
            "crates/a/src/lib.rs"
        );
        assert_eq!(
            short_path("crates/a", "crates/ab/lib.rs"),
            "crates/ab/lib.rs"
        );
    }
}
