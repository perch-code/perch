//! The tour's two sides. On the left, the outline: the start, each
//! chapter with its stops and their marks, and the end. On the right, the
//! code map: the changed packages and modules, each with its added and
//! removed lines, and the files in no module by tour package. Both light
//! up the stop in view, the last one whose top passed a third of the
//! column, and a click on either goes there. A module or file in no stop
//! is in italics and opens in Code.
//!
//! A narrow tab drops the code map first, then the outline.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Container, Decorators, Empty, Label, Stack};
use floem::{AnyView, ViewId};
use perch_changescape::codemap::{self, CodeMap, MapFile, Module, Other, Package};
use perch_changescape::page;
use perch_changescape::v4::{Status, Stop};
use perch_github::TourMark;

use crate::review::cards::tip;
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
/// A code map with at most this many top packages opens them all.
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
    let map = codemap::code_map(&cx.page);
    let shown = map.file_count() > 0;
    Stack::horizontal((
        outline(cx).style(move |s| s.apply_if(width.get() < OUTLINE_FROM, |s| s.hide())),
        Container::new(column).style(|s| {
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .height_full()
        }),
        code_map(cx, map)
            .style(move |s| s.apply_if(!shown || width.get() < MAP_FROM, |s| s.hide())),
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
            rows.push(stop_row(state, n, stop, page::folds(stop)));
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
fn stop_row(state: ReviewState, n: usize, stop: &Stop, skim: bool) -> AnyView {
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
                .chain(
                    stop.pictures
                        .iter()
                        .filter_map(|p| page::hunk(p).map(|(file, _)| file.to_string())),
                )
                .collect();
            (stop.id.clone(), paths)
        })
        .collect()
}

/// The code map of the page, drawn as its packages and modules, then the
/// files in no module by tour package.
fn code_map(cx: &Ctx, map: CodeMap) -> impl IntoView {
    let stops = Rc::new(stop_paths(cx));
    let most = map
        .packages
        .iter()
        .map(|p| p.adds + p.dels)
        .chain(map.other.iter().map(|o| o.adds + o.dels))
        .max()
        .unwrap_or(1)
        .max(1);
    let opened = RwSignal::new(if map.packages.len() <= MAP_OPEN_PACKAGES {
        map.packages
            .iter()
            .filter_map(|p| p.element.clone())
            .collect()
    } else {
        HashSet::<String>::new()
    });
    let mut rows: Vec<AnyView> = vec![Stack::horizontal((
        Label::new("Code map").style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_muted())
                .selectable(false)
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        side_label(format!("{} files", thousands(map.file_count() as i64))),
    ))
    .style(|s| s.items_center().width_full().padding_horiz(6.0))
    .into_any()];
    rows.push(
        Label::new("Packages and modules the change touches. The ones in the stop in view light up. A check means their stops are done.")
            .style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .width_full()
                    .padding_horiz(6.0)
                    .margin_bottom(6.0)
            })
            .into_any(),
    );
    for p in &map.packages {
        rows.push(package(cx, &stops, p, 0, most, opened));
    }
    for o in &map.other {
        rows.push(other(cx, &stops, o, most, opened));
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

/// What a map row stands for: the stops that hold its files, and whether
/// the stop in view is one of them, and whether they are all done.
#[derive(Clone)]
struct Reach {
    held: Vec<String>,
    here: Memo<bool>,
    done: Memo<bool>,
}

fn reach(
    state: ReviewState,
    stops: &Rc<Vec<(String, HashSet<String>)>>,
    paths: Vec<String>,
) -> Reach {
    let held: Vec<String> = stops
        .iter()
        .filter(|(_, held)| paths.iter().any(|p| held.contains(p)))
        .map(|(id, _)| id.clone())
        .collect();
    let here = {
        let stops = stops.clone();
        Memo::new(move |_| in_view(state, &stops, &paths))
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
    Reach { held, here, done }
}

/// A click on a row goes to the first stop that holds its files, or opens
/// the file in Code when no stop holds them.
fn open_reach(cx: &Ctx, reach: &Reach, path: &str) {
    match reach.held.first() {
        Some(stop) => go_to(cx.state, stop),
        None => navigate::open_file_line(cx.state, &cx.backend, path, None),
    }
}

/// The left margin of a map row at `depth`: a package's chevron and a
/// module's check sit in the first slot, so names line up after it.
fn indent(depth: usize) -> f64 {
    6.0 + 10.0 * depth as f64
}

/// The width of a map row's first slot, and the margin of its name.
const SLOT: f64 = 14.0;

fn chevron(open: Memo<bool>) -> Label {
    Label::derived(move || if open.get() { "▾" } else { "▸" }.to_string()).style(|s| {
        s.width(SLOT)
            .flex_shrink(0.0_f32)
            .font_size(theme::TEXT_BADGE)
            .color(theme::fg_subtle())
            .selectable(false)
    })
}

fn check(done: Memo<bool>) -> Label {
    Label::derived(move || if done.get() { "✓" } else { "" }.to_string()).style(|s| {
        s.width(SLOT)
            .flex_shrink(0.0_f32)
            .font_size(theme::TEXT_BADGE)
            .color(theme::fg_subtle())
            .selectable(false)
    })
}

/// The added and removed bars of a row, scaled to the largest row.
fn counts(adds: i64, dels: i64, most: i64) -> impl IntoView {
    let bar = |n: i64, color: fn() -> floem::peniko::Color| {
        let width = if n == 0 {
            0.0
        } else {
            (BAR_MAX * n as f64 / most as f64).max(2.0)
        };
        Empty::new().style(move |s| s.width(width).height(4.0).background(color()))
    };
    Stack::horizontal((
        bar(adds, theme::diff_added_bar),
        bar(dels, theme::diff_deleted_bar),
    ))
    .style(|s| {
        s.items_center()
            .gap(1.0)
            .margin_left(6.0)
            .flex_shrink(0.0_f32)
    })
}

/// The ink of a name by its status: added and removed names are colored.
fn status_ink(status: Status) -> fn() -> floem::peniko::Color {
    match status {
        Status::Added => theme::status_added,
        Status::Removed => theme::status_deleted,
        _ => theme::fg,
    }
}

/// A package: a chevron and its name, then its bars. A click opens it.
/// Its children sit under it, one step in.
fn package(
    cx: &Ctx,
    stops: &Rc<Vec<(String, HashSet<String>)>>,
    p: &Package,
    depth: usize,
    most: i64,
    opened: RwSignal<HashSet<String>>,
) -> AnyView {
    let state = cx.state;
    let reach = reach(state, stops, p.paths());
    let (here, done) = (reach.here, reach.done);
    // A group of modules in no package has no element: it is muted, and
    // it starts closed, as its key is never opened by default.
    let key = p
        .element
        .clone()
        .unwrap_or_else(|| format!("group:{}", p.name));
    let open = {
        let key = key.clone();
        Memo::new(move |_| here.get() || opened.with(|o| o.contains(&key)))
    };
    let ink: fn() -> floem::peniko::Color = match p.element {
        Some(_) => status_ink(p.status),
        None => theme::fg_subtle,
    };
    let name = match &p.element {
        Some(element) => format!("Changescape map package {element}"),
        None => format!("Changescape map package {}", p.name),
    };
    let header = Container::new(
        Stack::horizontal((
            chevron(open),
            Label::new(p.name.clone()).style(move |s| {
                s.font_size(theme::TEXT_META)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(ink())
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_shrink(1.0_f32)
                    .selectable(false)
            }),
            check(done),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            counts(p.adds, p.dels, most),
        ))
        .style(|s| s.items_center().width_full().min_width(0.0)),
    )
    .debug_name(name)
    .on_event_stop(el::Click, move |_, _| toggle(opened, &key))
    .style(move |s| {
        hover_row(
            s.margin_top(4.0)
                .padding_vert(3.0)
                .padding_left(indent(depth))
                .padding_right(6.0)
                .apply_if(here.get(), |s| s.background(theme::accent_subtle())),
        )
    });
    let mut rows: Vec<AnyView> = p
        .packages
        .iter()
        .map(|child| package(cx, stops, child, depth + 1, most, opened))
        .collect();
    rows.extend(
        p.modules
            .iter()
            .map(|m| module(cx, stops, m, depth + 1, most)),
    );
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

/// The first sentence of an element's detail, or None when it has none.
fn first_sentence(detail: &str) -> Option<&str> {
    let line = detail.trim().lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    Some(match line.find(". ") {
        Some(end) => &line[..=end],
        None => line,
    })
}

/// A module: its check, its name in the code font, and its bars. A click
/// goes to its first stop, or opens its file in Code. Its detail's first
/// sentence is a tip on hover.
fn module(
    cx: &Ctx,
    stops: &Rc<Vec<(String, HashSet<String>)>>,
    m: &Module,
    depth: usize,
    most: i64,
) -> AnyView {
    let state = cx.state;
    let reach = reach(state, stops, m.paths());
    let (here, done) = (reach.here, reach.done);
    let in_stop = !reach.held.is_empty();
    let ink = status_ink(m.status);
    let path = m.files.first().map(|f| f.path.clone()).unwrap_or_default();
    let open = {
        let (cx, reach) = (cx.clone(), reach.clone());
        move || open_reach(&cx, &reach, &path)
    };
    let row = Container::new(
        Stack::horizontal((
            check(done),
            Label::new(m.name.clone()).style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(ink())
                    .apply_if(!in_stop, |s| s.font_style(floem::text::FontStyle::Italic))
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_shrink(1.0_f32)
                    .selectable(false)
            }),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            counts(m.adds, m.dels, most),
        ))
        .style(|s| s.items_center().width_full().min_width(0.0)),
    )
    .debug_name(format!("Changescape map module {}", m.element))
    .on_event_stop(el::Click, move |_, _| open())
    .style(move |s| {
        hover_row(
            s.padding_vert(2.0)
                .padding_left(indent(depth))
                .padding_right(6.0)
                .apply_if(here.get(), |s| s.background(theme::accent_subtle())),
        )
    });
    let detail = cx
        .page
        .elements
        .get(&m.element)
        .and_then(|el| el.detail.as_deref())
        .and_then(first_sentence)
        .map(str::to_string);
    match detail {
        Some(text) => tip(row, text).into_any(),
        None => row.into_any(),
    }
}

/// The files in no module of one tour package, under a chevron. Opening
/// it shows the files.
fn other(
    cx: &Ctx,
    stops: &Rc<Vec<(String, HashSet<String>)>>,
    o: &Other,
    most: i64,
    opened: RwSignal<HashSet<String>>,
) -> AnyView {
    let key = format!("other:{}", o.name);
    let open = {
        let key = key.clone();
        Memo::new(move |_| opened.with(|o| o.contains(&key)))
    };
    let header = Container::new(
        Stack::horizontal((
            chevron(open),
            Label::new(format!("{} · {} files", o.name, o.files.len())).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_subtle())
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_shrink(1.0_f32)
                    .selectable(false)
            }),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            counts(o.adds, o.dels, most),
        ))
        .style(|s| s.items_center().width_full().min_width(0.0)),
    )
    .debug_name(format!("Changescape map other {}", o.name))
    .on_event_stop(el::Click, move |_, _| toggle(opened, &key))
    .style(|s| {
        hover_row(
            s.margin_top(4.0)
                .padding_vert(3.0)
                .padding_left(indent(0))
                .padding_right(6.0),
        )
    });
    let rows: Vec<AnyView> = o
        .files
        .iter()
        .map(|f| file_row(cx, stops, f, most, 1))
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

/// A file in an other group: its check, its folder muted and its name,
/// and its bars. A file in no stop is in italics and opens in Code.
fn file_row(
    cx: &Ctx,
    stops: &Rc<Vec<(String, HashSet<String>)>>,
    f: &MapFile,
    most: i64,
    depth: usize,
) -> AnyView {
    let state = cx.state;
    let reach = reach(state, stops, vec![f.path.clone()]);
    let (here, done) = (reach.here, reach.done);
    let in_stop = !reach.held.is_empty();
    let (folder, name) = match f.path.rsplit_once('/') {
        Some((folder, name)) => (format!("{folder}/"), name.to_string()),
        None => (String::new(), f.path.clone()),
    };
    let open = {
        let (cx, reach, path) = (cx.clone(), reach.clone(), f.path.clone());
        move || open_reach(&cx, &reach, &path)
    };
    Container::new(
        Stack::horizontal((
            check(done),
            Label::new(folder).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(theme::fg_subtle())
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_shrink(1.0_f32)
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
                    .flex_shrink(0.0_f32)
                    .selectable(false)
            }),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            counts(f.adds, f.dels, most),
        ))
        .style(|s| s.items_center().width_full().min_width(0.0)),
    )
    .debug_name(format!("Changescape map file {}", f.path))
    .on_event_stop(el::Click, move |_, _| open())
    .style(move |s| {
        hover_row(
            s.padding_vert(2.0)
                .padding_left(indent(depth))
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
    fn a_detail_shows_its_first_sentence() {
        assert_eq!(first_sentence(""), None);
        assert_eq!(first_sentence("  \n"), None);
        assert_eq!(
            first_sentence("Reads a page. Then it draws it."),
            Some("Reads a page.")
        );
        assert_eq!(first_sentence("No period here"), Some("No period here"));
        assert_eq!(
            first_sentence("Ends at a period."),
            Some("Ends at a period.")
        );
    }
}
