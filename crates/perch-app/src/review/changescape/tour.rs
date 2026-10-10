//! The tour: one scrolling column with a cover, the chapters, and an
//! ending. The cover has the summary, the bar of lines to read, skim and
//! skip, the flags, and the contents. Each chapter has its title, its
//! intro, its spine diagram, and its stops (see `stop`). The ending,
//! "Where you stand", lists what is left, the bands no stop holds, and
//! the checks and actions the page names.
//!
//! The page's line numbers count from the merge base, so the tour puts
//! the item on the Branch scope when it opens.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use floem::prelude::*;
use std::collections::HashSet;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use floem::AnyView;
use perch_changescape::page::{self, runs, Ref, Run, Svg, Verdict};
use perch_changescape::v3::{Tour, TourChapter};
use perch_changescape::LoadedPage;

use crate::backend::Backend;
use crate::review::changescape::prose::{self, hover_row, meta, BODY, HEADING, MUTED, STRONG};
use crate::review::changescape::side::{self, Placed};
use crate::review::changescape::{navigate, shown, stop, svg, view};
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::nav_row::thousands;
use crate::ui::widgets::{clickable, notice_box, underlined_link};

/// What every part of the tour needs.
#[derive(Clone)]
pub struct Ctx {
    pub app: AppState,
    pub state: ReviewState,
    pub backend: Rc<Backend>,
    pub page: Arc<LoadedPage>,
    /// Where the stops are, for the sides to find the one in view.
    pub placed: Placed,
}

impl Ctx {
    /// Open what an element id names: see `view::open_element`.
    pub fn open(&self, el: &str) {
        view::open_element(self, el);
    }

    pub fn tour(&self) -> &Tour {
        self.page
            .tour()
            .expect("the tour view draws a page with a tour")
    }

    /// Whether the agent wrote the tour's text.
    pub fn narrated(&self) -> bool {
        self.tour().narrated.unwrap_or(false)
    }
}

/// The tour of a page that has one, and over it a diagram Expand opened.
pub fn tour(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    page: Arc<LoadedPage>,
) -> impl IntoView {
    navigate::branch_scope(state, &backend);
    let cx = Ctx {
        app,
        state,
        backend,
        page,
        placed: Placed::default(),
    };
    let column = column(cx.clone()).into_any();
    Stack::vertical((
        side::sides(&cx, column),
        expanded(cx.clone()),
        view::popover(cx),
    ))
    .debug_name("Changescape tour")
    .style(|s| {
        s.flex_col()
            .size_full()
            .min_height(0.0)
            .position(floem::style::Position::Relative)
    })
}

fn column(cx: Ctx) -> impl IntoView {
    let (state, placed) = (cx.state, cx.placed.clone());
    let count = cx.tour().chapters.len();
    let mut parts: Vec<AnyView> = vec![cover(&cx).into_any()];
    for (n, chapter) in cx.tour().chapters.iter().enumerate() {
        parts.push(chapter_view(&cx, chapter.clone(), n + 1, count).into_any());
    }
    parts.push(ending(&cx).into_any());
    let scroll = Stack::vertical_from_iter(parts)
        .style(|s| {
            s.flex_col()
                .gap(40.0)
                .width(theme::CONVERSATION_WIDTH)
                .max_width_full()
        })
        .container()
        .style(|s| {
            s.width_full()
                .justify_center()
                .padding(theme::CONVERSATION_PADDING)
        })
        .scroll();
    let column = scroll.id();
    scroll
        .on_event_cont(
            floem::views::scroll::ScrollChanged::listener(),
            move |_, event: &floem::views::scroll::ScrollChanged| {
                side::track(state, &placed, column, event.offset.y);
            },
        )
        .debug_name("Changescape tour column")
        .style(|s| s.size_full())
}

/// A view that comes into view when the tour's focus names `id`, and
/// takes the focus.
pub fn focusable(state: ReviewState, id: String, view: AnyView) -> AnyView {
    let view_id = view.view_id();
    floem::reactive::Effect::new(move |_| {
        if state
            .changescape_focus
            .with(|f| f.as_deref() == Some(id.as_str()))
        {
            state.changescape_focus.set(None);
            // After the layout the asking caused, so there is a place to go.
            floem::action::exec_after(Duration::ZERO, move |_| view_id.scroll_to(None));
        }
    });
    view
}

/// Scroll the tour to a stop or a chapter, opening a skim stop in place.
pub fn go_to(state: ReviewState, id: &str) {
    let stop = state.changescape.with_untracked(|shown| {
        shown
            .as_ref()
            .and_then(|shown| shown.page())
            .is_some_and(|page| page.stop(id).is_some())
    });
    if stop && !state.changescape_opened.with_untracked(|o| o.contains(id)) {
        state.changescape_opened.update(|opened| {
            opened.insert(id.to_string());
        });
    }
    state.changescape_focus.set(Some(id.to_string()));
}

/// Add `id` to the set, or take it out when it is there.
pub fn toggle(set: RwSignal<HashSet<String>>, id: &str) {
    set.update(|set| {
        if !set.remove(id) {
            set.insert(id.to_string());
        }
    });
}

// ---------------------------------------------------------------------
// The cover
// ---------------------------------------------------------------------

fn cover(cx: &Ctx) -> AnyView {
    let tour = cx.tour();
    let mut parts: Vec<AnyView> = vec![Label::new(cx.page.meta.title.clone())
        .style(|s| {
            s.font_size(theme::TEXT_PAGE_TITLE)
                .font_weight(theme::weight_strong())
                .color(theme::fg())
                .line_height(theme::PAGE_TITLE_LINE_HEIGHT)
                .width_full()
        })
        .into_any()];
    if !cx.narrated() {
        let state = cx.state;
        parts.push(
            Label::derived(move || {
                let writing = shown(state).is_some_and(|shown| shown.narration_pending());
                if writing {
                    "The narration comes later: Claude Code is writing it now. Until then, the stops have no text, and their titles are made from the names in the code.".to_string()
                } else {
                    "The narration comes later. Until then, the stops have no text, and their titles are made from the names in the code.".to_string()
                }
            })
            .debug_name("Changescape narration pending")
            .style(|s| {
                notice_box(
                    s.font_size(theme::TEXT_META)
                        .color(theme::notice_fg())
                        .width_full()
                        .padding(10.0),
                )
            })
            .into_any(),
        );
    }
    if let Some(summary) = tour.summary.as_deref().filter(|s| !s.is_empty()) {
        parts.push(prose::paragraph(summary, BODY).into_any());
    }
    parts.push(lines_bar(tour).into_any());
    for flag in &tour.flags {
        parts.push(flag_view(cx, flag).into_any());
    }
    parts.push(contents(cx).into_any());
    let view = Stack::vertical_from_iter(parts)
        .debug_name("Changescape cover")
        .style(|s| s.flex_col().gap(14.0).width_full())
        .into_any();
    focusable(cx.state, side::COVER.to_string(), view)
}

/// The changed lines to read, to skim, and to skip, as one bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lines {
    pub read: i64,
    pub skim: i64,
    pub skip: i64,
}

/// The lines each chapter has to read and to skim, and the bands below
/// the stops by their verdict. A band of a verdict this Perch does not
/// know is skimmed.
pub fn lines(tour: &Tour) -> Lines {
    let mut lines = Lines::default();
    for chapter in &tour.chapters {
        if let Some(l) = &chapter.lines {
            lines.read += l.read;
            lines.skim += l.skim;
        }
    }
    for band in &tour.below {
        match band.verdict.as_str() {
            "skip" => lines.skip += band.lines,
            _ => lines.skim += band.lines,
        }
    }
    lines
}

fn lines_bar(tour: &Tour) -> impl IntoView {
    let lines = lines(tour);
    let total = (lines.read + lines.skim + lines.skip).max(1) as f64;
    let segment = |n: i64, color: fn() -> floem::peniko::Color| {
        let pct = n as f64 / total * 100.0;
        Empty::new().style(move |s| {
            s.width_pct(pct)
                .height_full()
                .background(color())
                .apply_if(n == 0, |s| s.hide())
        })
    };
    let legend = format!(
        "Read {} lines · skim {} · skip {}",
        thousands(lines.read),
        thousands(lines.skim),
        thousands(lines.skip)
    );
    Stack::vertical((
        Stack::horizontal((
            segment(lines.read, theme::fg),
            segment(lines.skim, theme::fg_subtle),
            segment(lines.skip, theme::border_muted),
        ))
        .style(|s| {
            s.width_full()
                .height(6.0)
                .border_radius(3.0)
                .background(theme::canvas_subtle())
        }),
        meta(legend),
    ))
    .debug_name("Changescape lines bar")
    .style(|s| s.flex_col().gap(6.0).width_full())
}

/// A flag: a fact that changes how to read the pull request, and a link
/// to what it points at.
fn flag_view(cx: &Ctx, flag: &perch_changescape::v3::TourFlag) -> impl IntoView {
    let warn = flag.kind.as_deref() == Some("warn");
    Stack::vertical((
        Stack::horizontal((
            Label::new(if warn { "⚠" } else { "ⓘ" }).style(move |s| {
                s.font_size(theme::TEXT_BODY)
                    .color(if warn {
                        theme::notice_fg()
                    } else {
                        theme::fg_muted()
                    })
                    .margin_right(8.0)
                    .selectable(false)
            }),
            prose::line(&flag.title, STRONG),
        ))
        .style(|s| s.items_center().width_full()),
        prose::paragraph(&flag.text, MUTED),
        reference(cx, flag.ref_.as_deref()),
    ))
    .debug_name("Changescape flag")
    .style(move |s| {
        let s = s.flex_col().gap(4.0).width_full().padding(12.0);
        if warn {
            notice_box(s)
        } else {
            s.background(theme::canvas_subtle())
                .border(1.0)
                .border_color(theme::border_muted())
                .border_radius(theme::RADIUS)
        }
    })
}

/// A link to what a ref names: a GitHub comment, which opens on GitHub,
/// a view, whose rows show in a popover, or an element, which opens as
/// `view::open_element` says. Nothing for a ref the page does not hold.
pub fn reference(cx: &Ctx, r: Option<&str>) -> AnyView {
    let Some(r) = r else {
        return Empty::new().into_any();
    };
    let (text, open): (String, Box<dyn Fn()>) = match page::parse_ref(r) {
        Ref::Comment(..) => {
            let Some(comment) = cx.page.comment(r) else {
                return Empty::new().into_any();
            };
            let url = comment.url.clone();
            (
                format!("The comment by @{}", comment.by),
                Box::new(move || crate::ui::widgets::open_in_browser(&url)),
            )
        }
        Ref::View(id) => {
            let Some(view) = cx.page.view(id) else {
                return Empty::new().into_any();
            };
            let (state, id) = (cx.state, id.to_string());
            (
                format!("See {}", view.t),
                Box::new(move || state.changescape_view.set(Some(id.clone()))),
            )
        }
        Ref::Element(el) => {
            let Some(element) = cx.page.elements.get(el) else {
                return Empty::new().into_any();
            };
            let (cx, el) = (cx.clone(), el.to_string());
            (
                format!("Open {}", element.t),
                Box::new(move || cx.open(&el)),
            )
        }
    };
    prose::link(text, open)
        .debug_name("Changescape ref")
        .into_any()
}

/// How many of an aside's or a check's rows show, before "+N more".
const ROWS_SHOWN: usize = 3;

/// The elements an aside or a check names, each a link that opens it: the
/// first few, then how many more. An element the page does not hold is
/// left out.
pub fn element_rows(cx: &Ctx, rows: &[String]) -> AnyView {
    let held: Vec<&String> = rows
        .iter()
        .filter(|el| cx.page.elements.contains_key(*el))
        .collect();
    if held.is_empty() {
        return Empty::new().into_any();
    }
    let mut parts: Vec<AnyView> = held
        .iter()
        .take(ROWS_SHOWN)
        .map(|el| {
            let title = cx.page.elements[*el].t.clone();
            let (cx, el) = (cx.clone(), el.to_string());
            Container::new(Label::new(title).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(theme::link())
                    .selectable(false)
            }))
            .debug_name("Changescape row element")
            .on_event_stop(el::Click, move |_, _| cx.open(&el))
            .style(|s| clickable(underlined_link(s)))
            .into_any()
        })
        .collect();
    if held.len() > ROWS_SHOWN {
        parts.push(
            Label::new(format!("+{} more", held.len() - ROWS_SHOWN))
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::fg_subtle())
                        .selectable(false)
                })
                .into_any(),
        );
    }
    Stack::horizontal_from_iter(parts)
        .debug_name("Changescape rows")
        .style(|s| {
            s.items_center()
                .flex_wrap(floem::style::FlexWrap::Wrap)
                .gap(10.0)
                .row_gap(2.0)
                .width_full()
        })
        .into_any()
}

/// The chapters, or the stops when there is one chapter, each a link to
/// where it is.
fn contents(cx: &Ctx) -> impl IntoView {
    let tour = cx.tour();
    let state = cx.state;
    let row = |id: String, title: String, count: String| {
        let go = id.clone();
        Container::new(
            Stack::horizontal((
                prose::line(&title, BODY),
                Empty::new().style(|s| s.flex_grow(1.0_f32)),
                meta(count).style(|s| s.margin_left(12.0).flex_shrink(0.0_f32)),
            ))
            .style(|s| s.items_center().width_full()),
        )
        .debug_name("Changescape contents row")
        .on_event_stop(el::Click, move |_, _| go_to(state, &go))
        .style(|s| hover_row(s.padding_vert(4.0).padding_horiz(6.0)))
        .into_any()
    };
    let rows: Vec<AnyView> = if tour.chapters.len() == 1 {
        let chapter = &tour.chapters[0];
        chapter
            .stops
            .iter()
            .map(|stop| {
                let skim = page::verdict(chapter, stop) == Verdict::Skim;
                let meta = format!(
                    "{} lines{}",
                    thousands(stop.lines),
                    if skim { " · skim" } else { "" }
                );
                row(stop.id.clone(), stop.title.clone(), meta)
            })
            .collect()
    } else {
        tour.chapters
            .iter()
            .map(|chapter| {
                let lines = chapter.lines.as_ref().map_or(0, |l| l.read + l.skim);
                let meta = format!("{} stops · {} lines", chapter.stops.len(), thousands(lines));
                row(chapter.id.clone(), chapter.title.clone(), meta)
            })
            .collect()
    };
    Stack::vertical((
        Label::new("Contents").style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_muted())
                .margin_bottom(4.0)
        }),
        Stack::vertical_from_iter(rows).style(|s| s.flex_col().width_full()),
    ))
    .debug_name("Changescape contents")
    .style(|s| s.flex_col().width_full().margin_top(8.0))
}

// ---------------------------------------------------------------------
// Chapters
// ---------------------------------------------------------------------

fn chapter_view(cx: &Ctx, chapter: TourChapter, n: usize, of: usize) -> AnyView {
    let mut parts: Vec<AnyView> = Vec::new();
    if of > 1 {
        parts.push(meta(format!("Chapter {n} of {of}")).into_any());
    }
    parts.push(prose::paragraph(&chapter.title, HEADING).into_any());
    if let Some(intro) = chapter.intro.as_deref().filter(|s| !s.is_empty()) {
        parts.push(prose::paragraph(intro, BODY).into_any());
    }
    if let Some(spine) = &chapter.spine {
        if let Some(svg) = page::spine_svg(spine) {
            parts.push(
                diagram(
                    cx,
                    spine_key(&chapter.id),
                    spine.caption.as_deref().or(spine.title.as_deref()),
                    spine.note.as_deref(),
                    svg,
                )
                .into_any(),
            );
        }
    }
    let mut stops: Vec<AnyView> = Vec::new();
    for run in runs(&chapter) {
        stops.push(match run {
            Run::Read(i) => stop::stop_card(cx, &chapter, i).into_any(),
            Run::Skim(group) => stop::skim_group(cx, &chapter, group).into_any(),
            Run::Section(k) => section_header(&chapter, k),
        });
    }
    parts.push(
        Stack::vertical_from_iter(stops)
            .style(|s| s.flex_col().gap(16.0).width_full().margin_top(8.0))
            .into_any(),
    );
    let view = Stack::vertical_from_iter(parts)
        .debug_name(format!("Changescape chapter {}", chapter.id))
        .style(|s| s.flex_col().gap(10.0).width_full())
        .into_any();
    focusable(cx.state, chapter.id.clone(), view)
}

/// The header of a chapter's section `k`, before its first stop: where it
/// is among the sections, its title, and its intro.
fn section_header(chapter: &TourChapter, k: usize) -> AnyView {
    let section = &chapter.sections[k];
    Stack::vertical((
        Label::new(format!("Section {} of {}", k + 1, chapter.sections.len())).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_subtle())
                .selectable(false)
        }),
        prose::paragraph(&section.title, prose::TITLE),
        section
            .intro
            .as_deref()
            .filter(|i| !i.is_empty())
            .map_or_else(
                || Empty::new().into_any(),
                |intro| prose::paragraph(intro, MUTED).into_any(),
            ),
    ))
    .debug_name(format!("Changescape section {}", section.title))
    .style(|s| {
        s.flex_col()
            .gap(4.0)
            .width_full()
            .margin_top(20.0)
            .padding_top(12.0)
            .border_top(1.0)
            .border_color(theme::border_muted())
    })
    .into_any()
}

/// A diagram of the tour: its heading, the picture, and its legend.
pub fn diagram(
    cx: &Ctx,
    key: String,
    heading: Option<&str>,
    note: Option<&str>,
    svg: Svg,
) -> impl IntoView {
    let heading = heading.filter(|h| !h.is_empty()).map_or_else(
        || Empty::new().into_any(),
        |h| prose::paragraph(h, MUTED).into_any(),
    );
    let note = note.filter(|n| !n.is_empty()).map_or_else(
        || Empty::new().into_any(),
        |n| {
            Label::new(prose::plain(n))
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::fg_subtle())
                        .width_full()
                })
                .into_any()
        },
    );
    Stack::vertical((
        heading,
        svg::svg_block(cx.clone(), svg, svg::Frame::Block(key)),
        note,
    ))
    .debug_name("Changescape picture")
    .style(|s| s.flex_col().gap(6.0).width_full())
}

/// The key of a chapter's spine.
pub fn spine_key(chapter: &str) -> String {
    format!("spine:{chapter}")
}

/// The key of a stop's picture.
pub fn picture_key(stop: &str, i: usize) -> String {
    format!("stop:{i}:{stop}")
}

/// The diagram a key names, with its heading.
pub fn picture(page: &LoadedPage, key: &str) -> Option<(String, Svg)> {
    let tour = page.tour()?;
    if let Some(chapter) = key.strip_prefix("spine:") {
        let spine = tour
            .chapters
            .iter()
            .find(|c| c.id == chapter)?
            .spine
            .as_ref()?;
        let title = spine
            .caption
            .clone()
            .or(spine.title.clone())
            .unwrap_or_default();
        return Some((prose::plain(&title), page::spine_svg(spine)?));
    }
    let (i, stop) = key.strip_prefix("stop:")?.split_once(':')?;
    let (_, stop) = page.stop(stop)?;
    let picture = stop.pictures.get(i.parse::<usize>().ok()?)?;
    let title = picture
        .caption
        .clone()
        .or(picture.title.clone())
        .unwrap_or_else(|| stop.title.clone());
    Some((prose::plain(&title), page::picture_svg(picture)?))
}

/// The diagram Expand opened, over the whole tour and fitted to it, with
/// its own zoom and scroll. The commit bar stays.
fn expanded(cx: Ctx) -> impl IntoView {
    let state = cx.state;
    dyn_container(
        move || state.changescape_expanded.get(),
        move |key| {
            let Some((title, svg)) = key.and_then(|key| picture(&cx.page, &key)) else {
                return Empty::new().into_any();
            };
            Stack::vertical((
                Label::new(title).style(|s| {
                    s.font_size(theme::TEXT_SECTION_TITLE)
                        .font_weight(theme::weight_strong())
                        .color(theme::fg())
                }),
                Container::new(svg::svg_block(cx.clone(), svg, svg::Frame::Expanded))
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

// ---------------------------------------------------------------------
// Progress and the ending
// ---------------------------------------------------------------------

/// "3 of 12 read stops done". Tracked.
pub fn progress_line(state: ReviewState, page: &LoadedPage) -> String {
    let Some(tour) = page.tour() else {
        return String::new();
    };
    let (done, all) = state.changescape_marks.with(|marks| {
        page::read_stops(tour).fold((0, 0), |(done, all), stop| {
            let is_done = marks.get(&stop.id) == Some(&perch_github::TourMark::Done);
            (done + usize::from(is_done), all + 1)
        })
    });
    format!("{done} of {all} read stops done")
}

fn ending(cx: &Ctx) -> AnyView {
    let tour = cx.tour();
    let state = cx.state;
    let section = |title: &'static str| {
        Label::new(title)
            .style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg_muted())
                    .margin_top(10.0)
            })
            .into_any()
    };
    let mut parts: Vec<AnyView> = vec![prose::paragraph("Where you stand", HEADING).into_any()];
    // What is left: the stops that changed since their marks, those to
    // come back to, and the read stops with no mark, as marks change.
    let page = cx.page.clone();
    parts.push(
        dyn_container(
            move || {
                (
                    state.changescape_marks.get(),
                    state.changescape_changed.get(),
                )
            },
            move |(marks, changed)| {
                let Some(tour) = page.tour() else {
                    return Empty::new().into_any();
                };
                let changed: Vec<_> = page::stops(tour)
                    .map(|(_, s)| s)
                    .filter(|s| changed.contains_key(&s.id))
                    .collect();
                let later: Vec<_> = page::stops(tour)
                    .map(|(_, s)| s)
                    .filter(|s| marks.get(&s.id) == Some(&perch_github::TourMark::Later))
                    .collect();
                let left: Vec<_> = page::read_stops(tour)
                    .filter(|s| !marks.contains_key(&s.id) && !changed.iter().any(|c| c.id == s.id))
                    .collect();
                let list = |stops: Vec<&perch_changescape::v3::TourStop>| {
                    Stack::vertical_from_iter(stops.into_iter().map(|stop| {
                        let id = stop.id.clone();
                        Container::new(prose::line(&stop.title, BODY))
                            .debug_name("Changescape left row")
                            .on_event_stop(el::Click, move |_, _| go_to(state, &id))
                            .style(|s| hover_row(s.padding_vert(3.0).padding_horiz(6.0)))
                            .into_any()
                    }))
                    .style(|s| s.flex_col().width_full())
                    .into_any()
                };
                let mut parts: Vec<AnyView> = Vec::new();
                if !changed.is_empty() {
                    parts.push(section("Changed since you marked them"));
                    parts.push(list(changed));
                }
                if !later.is_empty() {
                    parts.push(section("Come back to these"));
                    parts.push(list(later));
                }
                if left.is_empty() {
                    parts.push(
                        Label::new("Every read stop is marked.")
                            .style(|s| s.font_size(theme::TEXT_BODY).color(theme::fg_muted()))
                            .into_any(),
                    );
                } else {
                    parts.push(section("Read stops with no mark"));
                    parts.push(list(left));
                }
                Stack::vertical_from_iter(parts)
                    .style(|s| s.flex_col().gap(4.0).width_full())
                    .into_any()
            },
        )
        .debug_name("Changescape left")
        .style(|s| s.width_full())
        .into_any(),
    );
    if !tour.below.is_empty() {
        parts.push(section("In no stop"));
        for band in &tour.below {
            let label = band.label.clone().unwrap_or_else(|| band.part.clone());
            let files: Vec<String> = band.files.iter().map(|f| f.path.clone()).collect();
            parts.push(
                Stack::vertical((
                    Label::new(format!(
                        "{label} · {} · {} lines",
                        band.verdict,
                        thousands(band.lines)
                    ))
                    .style(|s| s.font_size(theme::TEXT_BODY).color(theme::fg())),
                    Label::new(files.join(", ")).style(|s| {
                        s.font_size(theme::TEXT_BADGE)
                            .font_family(theme::CODE_FONT_STACK.to_string())
                            .color(theme::fg_subtle())
                            .width_full()
                    }),
                ))
                .debug_name("Changescape band")
                .style(|s| s.flex_col().gap(2.0).width_full())
                .into_any(),
            );
        }
    }
    let (checks, actions) = tour
        .ending
        .as_ref()
        .map(|e| (e.checks.clone(), e.actions.clone()))
        .unwrap_or_default();
    if !checks.is_empty() {
        parts.push(section("Before you decide"));
        for check in checks {
            parts.push(
                Stack::vertical((
                    prose::paragraph(&check.q, STRONG),
                    prose::paragraph(&check.a, MUTED),
                    element_rows(cx, &check.rows),
                ))
                .debug_name("Changescape check")
                .style(|s| s.flex_col().width_full())
                .into_any(),
            );
        }
    }
    if !actions.is_empty() {
        parts.push(section("Next"));
        for action in actions {
            parts.push(
                Stack::vertical((
                    prose::paragraph(&action.label, STRONG),
                    action.detail.as_deref().map_or_else(
                        || Empty::new().into_any(),
                        |d| prose::paragraph(d, MUTED).into_any(),
                    ),
                ))
                .debug_name("Changescape action")
                .style(|s| s.flex_col().width_full())
                .into_any(),
            );
        }
    }
    let view = Stack::vertical_from_iter(parts)
        .debug_name("Changescape ending")
        .style(|s| s.flex_col().gap(6.0).width_full().padding_bottom(40.0))
        .into_any();
    cx.placed.add(None, view.view_id());
    focusable(state, side::ENDING.to_string(), view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_counts_chapters_and_bands_by_verdict() {
        let tour: Tour = serde_json::from_value(serde_json::json!({
            "chapters": [
                { "id": "a", "title": "A", "stops": [], "lines": { "read": 10, "skim": 4 } },
                { "id": "b", "title": "B", "stops": [], "lines": { "read": 5, "skim": 0 } },
            ],
            "below": [
                { "part": "docs", "verdict": "skim", "lines": 3 },
                { "part": "generated", "verdict": "skip", "lines": 100 },
                { "part": "odd", "verdict": "glance", "lines": 1 },
            ],
        }))
        .unwrap();
        assert_eq!(
            lines(&tour),
            Lines {
                read: 15,
                skim: 8,
                skip: 100
            }
        );
    }
}
