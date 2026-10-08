//! A view on the Changescape tab: its title, its caption, and its blocks.
//! Text, summaries and tables are drawn here; diagrams in `svg`. A block of
//! a type Perch does not draw is one quiet line that offers the browser
//! page, which draws every type.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout};
use floem::views::{Container, Decorators, Empty, Label, Stack};
use floem::AnyView;
use perch_changescape::page::{Block, BlockBody, Cell, Column, Row, Summary, Table, Text, View};
use perch_changescape::Page;

use crate::backend::Backend;
use crate::review::cards::tip_with;
use crate::review::changescape::{navigate, show_section, show_view, shown};
use crate::review::linked_text::{linked_text, Target};
use crate::review::model::ReviewState;
use crate::theme;
use crate::ui::widgets::{clickable, underlined_link};

/// What every block needs to open what it names.
#[derive(Clone)]
pub struct Ctx {
    pub state: ReviewState,
    pub backend: Rc<Backend>,
    pub page: Arc<Page>,
    /// The open review threads on each element.
    pub counts: floem::reactive::Memo<Arc<super::threads::Counts>>,
}

impl Ctx {
    /// The badge of an element's open threads.
    pub fn badge(&self, el: &str) -> impl IntoView {
        super::threads::badge(
            self.state,
            self.backend.clone(),
            self.counts,
            el.to_string(),
        )
    }
}

impl Ctx {
    pub fn open(&self, el: &str) {
        navigate::open_element(self.state, &self.backend, &self.page, el);
    }
}

pub fn view(cx: Ctx, view: View, titled: bool) -> impl IntoView {
    let mut parts: Vec<AnyView> = Vec::new();
    if titled {
        parts.push(
            Stack::horizontal((
                Label::new(view.t.clone()).style(|s| {
                    s.font_size(theme::TEXT_SECTION_TITLE)
                        .font_weight(theme::weight_strong())
                        .color(theme::fg())
                }),
                Label::new(view.metric.clone()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_muted())
                        .margin_left(10.0)
                }),
            ))
            .style(|s| s.items_baseline())
            .into_any(),
        );
    }
    if titled && !view.s.is_empty() {
        parts.push(
            Label::new(view.s.clone())
                .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_muted()))
                .into_any(),
        );
    }
    if let Some(caption) = view.narration.clone().filter(|c| !c.is_empty()) {
        parts.push(paragraph(&cx, &caption, &[], None).into_any());
    }
    for block in view.blocks {
        parts.push(block_view(cx.clone(), block).into_any());
    }
    Stack::vertical_from_iter(parts)
        .debug_name(format!("Changescape view {}", view.id))
        .style(|s| s.flex_col().gap(8.0).width_full())
}

fn block_view(cx: Ctx, block: Block) -> impl IntoView {
    let kind = block.kind.clone();
    let body = match &block.body {
        BlockBody::Text(text) => text_block(&cx, text).into_any(),
        BlockBody::Summary(summary) => summary_block(&cx, summary).into_any(),
        BlockBody::Table(table) => table_block(&cx, table).into_any(),
        BlockBody::Svg(svg) => super::svg::svg_block(cx.clone(), svg.clone()),
        BlockBody::Other => fallback(cx.state).into_any(),
    };
    let mut parts: Vec<AnyView> = Vec::new();
    if let Some(title) = block.title.clone() {
        parts.push(
            Label::new(title)
                .style(|s| {
                    s.font_size(theme::TEXT_SECTION)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg_muted())
                })
                .into_any(),
        );
    }
    parts.push(body);
    if let Some(note) = block.note.clone() {
        parts.push(
            Label::new(note)
                .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_subtle()))
                .into_any(),
        );
    }
    Stack::vertical_from_iter(parts)
        .debug_name(format!("Changescape block {kind}"))
        .style(|s| s.flex_col().gap(6.0).width_full().margin_top(8.0))
}

/// A paragraph: its spans when the file splits it into them, else its
/// text; linked whole by the block's `el`.
fn text_block(cx: &Ctx, text: &Text) -> impl IntoView {
    let pieces: Vec<(String, Option<String>)> = if text.spans.is_empty() {
        vec![(text.text.clone(), None)]
    } else {
        text.spans
            .iter()
            .map(|span| (span.text.clone(), span.el.clone()))
            .collect()
    };
    let joined: String = pieces.iter().map(|(text, _)| text.as_str()).collect();
    let mut links = Vec::new();
    let mut at = 0;
    for (piece, el) in &pieces {
        if let Some(el) = el {
            links.push((at..at + piece.len(), el.clone()));
        }
        at += piece.len();
    }
    paragraph(cx, &joined, &links, text.el.as_deref())
}

/// Prose with `code` spans in backticks. `links` are byte ranges of `text`
/// with the element each opens; `whole` links all of it.
fn paragraph(
    cx: &Ctx,
    text: &str,
    links: &[(Range<usize>, String)],
    whole: Option<&str>,
) -> impl IntoView {
    let (plain, code, map) = backticks(text);
    let moved = |range: &Range<usize>| map(range.start)..map(range.end);
    let mut targets: Vec<(Range<usize>, Target)> = Vec::new();
    match whole {
        Some(el) => targets.push((0..plain.len(), element_target(cx, el))),
        None => {
            for (range, el) in links {
                let range = moved(range);
                if !range.is_empty() {
                    targets.push((range, element_target(cx, el)));
                }
            }
        }
    }
    let link_ranges: Vec<Range<usize>> = targets.iter().map(|(range, _)| range.clone()).collect();
    let len = plain.len();
    linked_text(plain, targets, move || {
        (prose_attrs(len, &code, &link_ranges), Vec::new())
    })
    .style(|s| s.width_full())
}

/// The text without its backticks, the byte ranges that were in them, and
/// a map from a byte offset in `text` to one in the result.
fn backticks(text: &str) -> (String, Vec<Range<usize>>, impl Fn(usize) -> usize) {
    let mut plain = String::with_capacity(text.len());
    let mut code = Vec::new();
    let mut offsets = Vec::with_capacity(text.len() + 1);
    let mut open: Option<usize> = None;
    for (i, ch) in text.char_indices() {
        while offsets.len() <= i {
            offsets.push(plain.len());
        }
        if ch == '`' {
            match open.take() {
                Some(start) => code.push(start..plain.len()),
                None => open = Some(plain.len()),
            }
            continue;
        }
        plain.push(ch);
    }
    while offsets.len() <= text.len() {
        offsets.push(plain.len());
    }
    // An unclosed backtick is a backtick.
    if let Some(start) = open {
        plain.insert(start, '`');
        for offset in offsets.iter_mut().filter(|o| **o > start) {
            *offset += 1;
        }
    }
    (plain, code, move |i: usize| {
        offsets[i.min(offsets.len() - 1)]
    })
}

fn prose_attrs(len: usize, code: &[Range<usize>], links: &[Range<usize>]) -> AttrsList {
    let prose: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::FONT_STACK).collect();
    let mono: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect();
    let words = || {
        Attrs::new()
            .family(&prose)
            .font_size(theme::TEXT_BODY)
            .weight(theme::weight_read())
            .line_height(floem::text::LineHeightValue::Normal(
                theme::PROSE_LINE_HEIGHT,
            ))
            .color(theme::fg())
    };
    let mut list = AttrsList::new(words());
    for range in links.iter().filter(|r| r.end <= len) {
        list.add_span(range.clone(), words().color(theme::link()));
    }
    for range in code.iter().filter(|r| r.end <= len) {
        let linked = links
            .iter()
            .any(|l| l.start <= range.start && range.end <= l.end);
        list.add_span(
            range.clone(),
            Attrs::new()
                .family(&mono)
                .font_size(theme::CODE_FONT_SIZE)
                .weight(theme::weight_body())
                .color(if linked { theme::link() } else { theme::fg() }),
        );
    }
    list
}

fn element_target(cx: &Ctx, el: &str) -> Target {
    let tip = cx
        .page
        .elements
        .get(el)
        .map(|element| element.t.clone())
        .unwrap_or_else(|| el.to_string());
    let cx = cx.clone();
    let el = el.to_string();
    Target::Local {
        tip,
        open: Rc::new(move || cx.open(&el)),
    }
}

/// The overview's rows: a title, a metric, and a line, each opening its
/// view's section.
fn summary_block(cx: &Ctx, summary: &Summary) -> impl IntoView {
    let rows: Vec<AnyView> = summary
        .items
        .iter()
        .map(|item| {
            let (state, page) = (cx.state, cx.page.clone());
            let (view, sec) = (item.view.clone(), item.sec.clone());
            Stack::horizontal((
                Label::new(item.title.clone()).style(|s| {
                    s.font_size(theme::TEXT_BODY)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg())
                        .width(160.0)
                        .flex_shrink(0.0_f32)
                }),
                Stack::vertical((
                    Label::new(item.metric.clone()).style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::fg_muted())
                            .width_full()
                            .min_width(0.0)
                    }),
                    Label::new(item.text.clone()).style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::fg())
                            .width_full()
                            .min_width(0.0)
                    }),
                ))
                .style(|s| {
                    s.flex_col()
                        .gap(2.0)
                        .flex_grow(1.0_f32)
                        .flex_basis(0.0)
                        .min_width(0.0)
                }),
            ))
            .debug_name("Changescape summary row")
            .on_event_stop(el::Click, move |_, _| match (&sec, &view) {
                (Some(sec), _) => show_section(state, sec),
                (None, Some(view)) => show_view(state, &page, view),
                (None, None) => {}
            })
            .style(|s| {
                clickable(
                    s.items_start()
                        .width_full()
                        .padding_vert(8.0)
                        .padding_horiz(8.0)
                        .border_radius(theme::RADIUS),
                )
                .hover(|s| s.background(theme::canvas_hover()))
            })
            .into_any()
        })
        .collect();
    Stack::vertical_from_iter(rows).style(|s| s.flex_col().width_full())
}

/// How wide a column of each kind is, where the kind fixes it.
fn column_width(kind: &str) -> Option<f64> {
    match kind {
        "num" => Some(64.0),
        "status" => Some(88.0),
        "bar" => Some(96.0),
        _ => None,
    }
}

/// How a column takes its room in a row.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Width {
    /// As wide as this, given or shrinking.
    Fixed(f64),
    /// Its content's width as it starts, giving way in proportion to it
    /// when the row runs short, but not below `MIN_TEXT`.
    Content(f64),
    /// The whole of what holds it: one entry of a refs cell.
    Fill,
}

/// Room each side of a cell's text.
const CELL_PAD: f64 = 6.0;
/// A column whose widest text is this narrow keeps its width: a kind, a
/// hash, a count.
const SNUG: f64 = 140.0;
/// The narrowest a text column gives way to.
const MIN_TEXT: f64 = 96.0;
/// The widest a text column asks for to start with, so one long message
/// does not take the row from the others.
const MAX_TEXT: f64 = 520.0;

/// How wide each of a table's columns is: a fixed kind keeps its width; a
/// text column starts at the width of its widest text, the header's
/// among them.
fn column_widths(table: &Table) -> Vec<Width> {
    table
        .columns
        .iter()
        .map(|column| {
            if let Some(width) = column_width(&column.kind) {
                return Width::Fixed(width);
            }
            let mono = column.kind == "mono" || column.kind == "refs";
            let widest = table
                .rows
                .iter()
                .filter_map(|row| row.cells.get(&column.key))
                .flat_map(|cell| {
                    if cell.items.is_empty() {
                        vec![cell_text_width(&cell.text, mono)]
                    } else {
                        cell.items
                            .iter()
                            .map(|item| cell_text_width(&item.text, true))
                            .collect()
                    }
                })
                .fold(header_width(&column.label), f64::max);
            // A little over, for the link's underline and rounding.
            let width = widest.ceil() + 2.0 * CELL_PAD + 2.0;
            if width <= SNUG {
                Width::Fixed(width)
            } else {
                Width::Content(width.min(MAX_TEXT))
            }
        })
        .collect()
}

fn header_width(label: &str) -> f64 {
    measure(label, theme::FONT_STACK, theme::TEXT_SECTION, true)
}

fn cell_text_width(text: &str, mono: bool) -> f64 {
    if mono {
        measure(text, theme::CODE_FONT_STACK, theme::CODE_FONT_SIZE, false)
    } else {
        measure(text, theme::FONT_STACK, theme::TEXT_META, false)
    }
}

/// How wide `text` is on one line in `stack` at `size`.
fn measure(text: &str, stack: &str, size: f32, emphasis: bool) -> f64 {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list(stack).collect();
    let mut attrs = Attrs::new().font_size(size).family(&family);
    if emphasis {
        attrs = attrs.weight(theme::weight_emphasis());
    }
    TextLayout::new_with_text(text, AttrsList::new(attrs), None)
        .size()
        .width
}

fn column_style(s: floem::style::Style, width: Width) -> floem::style::Style {
    let s = s.padding_horiz(CELL_PAD).min_width(0.0);
    match width {
        Width::Fixed(width) => s.width(width).flex_shrink(0.0_f32),
        // Shrinking in proportion to its basis, as a flex item does, so
        // each text column gives up room as it asked for it.
        Width::Content(width) => s
            .flex_basis(width)
            .flex_grow(1.0_f32)
            .flex_shrink(1.0_f32)
            .min_width(MIN_TEXT),
        Width::Fill => s.width_full(),
    }
}

fn table_block(cx: &Ctx, table: &Table) -> impl IntoView {
    let columns = Rc::new(table.columns.clone());
    let widths = Rc::new(column_widths(table));
    // The longest bar in a column is full.
    let maxima: Rc<Vec<f64>> = Rc::new(
        columns
            .iter()
            .map(|column| {
                table
                    .rows
                    .iter()
                    .filter_map(|row| row.cells.get(&column.key)?.value)
                    .fold(0.0_f64, f64::max)
            })
            .collect(),
    );
    let badges = table.rows.iter().any(|row| row.el.is_some());
    let header = Stack::horizontal_from_iter(
        columns
            .iter()
            .zip(widths.iter().copied())
            .map(|(column, width)| {
                let right = column.kind == "num";
                Label::new(column.label.clone())
                    .style(move |s| {
                        column_style(s, width)
                            .font_size(theme::TEXT_SECTION)
                            .font_weight(theme::weight_emphasis())
                            .color(theme::fg_muted())
                            .apply_if(right, |s| s.justify_end())
                    })
                    .into_any()
            })
            .chain(badges.then(|| {
                Empty::new()
                    .style(|s| s.width(BADGE_SLOT).flex_shrink(0.0_f32))
                    .into_any()
            })),
    )
    .style(|s| {
        s.width_full()
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(theme::border_muted())
    });
    let rows: Vec<AnyView> = table
        .rows
        .iter()
        .map(|row| table_row(cx, &columns, &widths, &maxima, row, badges).into_any())
        .collect();
    Stack::vertical((
        header,
        Stack::vertical_from_iter(rows).style(|s| s.flex_col().width_full()),
    ))
    .style(|s| s.flex_col().width_full())
}

/// Room at a row's end for the badge of its threads.
const BADGE_SLOT: f64 = 36.0;

fn table_row(
    cx: &Ctx,
    columns: &Rc<Vec<Column>>,
    widths: &Rc<Vec<Width>>,
    maxima: &Rc<Vec<f64>>,
    row: &Row,
    badges: bool,
) -> impl IntoView {
    let dim = row.dim;
    let mut cells: Vec<AnyView> = columns
        .iter()
        .enumerate()
        .map(|(i, column)| {
            let cell = row.cells.get(&column.key).cloned().unwrap_or_default();
            table_cell(cx, column, widths[i], maxima[i], cell, dim).into_any()
        })
        .collect();
    if badges {
        let badge = match &row.el {
            Some(el) => cx.badge(el).into_any(),
            None => Empty::new().into_any(),
        };
        cells.push(
            Container::new(badge)
                .style(|s| s.width(BADGE_SLOT).flex_shrink(0.0_f32).justify_end())
                .into_any(),
        );
    }
    let line = Stack::horizontal_from_iter(cells);
    let line = match row.el.clone() {
        Some(el) => {
            let cx = cx.clone();
            line.on_event_stop(el::Click, move |_, _| cx.open(&el))
                .style(|s| clickable(row_style(s)).hover(|s| s.background(theme::canvas_hover())))
                .into_any()
        }
        None => line.style(row_style).into_any(),
    };
    Container::new(line)
        .debug_name("Changescape row")
        .style(|s| s.width_full())
}

fn row_style(s: floem::style::Style) -> floem::style::Style {
    s.width_full()
        .items_center()
        .min_height(theme::SUMMARY_ROW)
        .border_bottom(1.0)
        .border_color(theme::border_muted())
}

fn table_cell(cx: &Ctx, column: &Column, width: Width, max: f64, cell: Cell, dim: bool) -> AnyView {
    let kind = column.kind.clone();
    let ink = move |status: &str| {
        if dim {
            return theme::fg_subtle();
        }
        match status {
            "added" => theme::status_added(),
            "removed" | "deleted" => theme::status_deleted(),
            "changed" | "modified" => theme::status_modified(),
            "moved" | "renamed" => theme::status_renamed(),
            _ => theme::fg_muted(),
        }
    };
    let body: AnyView = match kind.as_str() {
        _ if !cell.items.is_empty() => {
            let links: Vec<AnyView> = cell
                .items
                .iter()
                .map(|item| {
                    let one = Column {
                        kind: "mono".into(),
                        ..column.clone()
                    };
                    Container::new(table_cell(cx, &one, Width::Fill, max, item.clone(), dim))
                        .style(|s| s.width_full().min_width(0.0))
                        .into_any()
                })
                .collect();
            Stack::vertical_from_iter(links)
                .style(|s| {
                    s.flex_col()
                        .gap(2.0)
                        .width_full()
                        .min_width(0.0)
                        .padding_vert(4.0)
                })
                .into_any()
        }
        "bar" => {
            let share = match cell.value {
                Some(value) if max > 0.0 => (value / max).clamp(0.0, 1.0),
                _ => 0.0,
            };
            Container::new(Empty::new().style(move |s| {
                s.height(6.0)
                    .width_pct(share * 100.0)
                    .border_radius(3.0)
                    .background(if dim {
                        theme::fg_subtle()
                    } else {
                        theme::accent()
                    })
            }))
            .style(|s| {
                s.width_full()
                    .height(6.0)
                    .border_radius(3.0)
                    .background(theme::canvas_subtle())
            })
            .into_any()
        }
        _ => {
            let text = cell.text.clone();
            let mono = kind == "mono" || kind == "refs";
            let status = kind == "status";
            // Prose wraps in its column. A name or a path stays on its
            // line, cut short where the column is narrower, with the whole
            // of it in a tip.
            let cut = mono || status;
            let linked = cell.el.is_some() || cell.file.is_some();
            let status_text = text.clone();
            let label = Label::new(text).style(move |s| {
                let s = s
                    .font_size(if mono {
                        theme::CODE_FONT_SIZE
                    } else {
                        theme::TEXT_META
                    })
                    .min_width(0.0)
                    .apply_if(cut, |s| s.text_ellipsis())
                    .apply_if(!cut, |s| s.text_wrap().width_full())
                    .color(if status {
                        ink(&status_text)
                    } else if linked && !dim {
                        theme::link()
                    } else if dim {
                        theme::fg_subtle()
                    } else {
                        theme::fg()
                    });
                if mono {
                    s.font_family(theme::CODE_FONT_STACK.to_string())
                } else {
                    s
                }
            });
            let label: AnyView = if mono && !cell.text.is_empty() {
                let whole = cell.text.clone();
                tip_with(label, move || whole.clone()).into_any()
            } else {
                label.into_any()
            };
            match (cell.el.clone(), cell.file.clone()) {
                (Some(el), _) => {
                    let cx = cx.clone();
                    Container::new(label)
                        .on_event_stop(el::Click, move |_, _| cx.open(&el))
                        .style(move |s| {
                            underlined_link(s)
                                .min_width(0.0)
                                .max_width_full()
                                .apply_if(!cut, |s| s.width_full())
                        })
                        .into_any()
                }
                (None, Some(file)) => {
                    let (state, backend, line) = (cx.state, cx.backend.clone(), cell.line);
                    Container::new(label)
                        .on_event_stop(el::Click, move |_, _| {
                            navigate::open_file_line(state, &backend, &file, line)
                        })
                        .style(move |s| {
                            underlined_link(s)
                                .min_width(0.0)
                                .max_width_full()
                                .apply_if(!cut, |s| s.width_full())
                        })
                        .into_any()
                }
                (None, None) => label,
            }
        }
    };
    let right = kind == "num";
    Container::new(body)
        .style(move |s| {
            column_style(s, width)
                .items_center()
                .apply_if(right, |s| s.justify_end())
        })
        .into_any()
}

/// A block Perch does not draw: one line, and the browser page.
fn fallback(state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        Label::new("This view is in the browser page.").style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_subtle())
                .selectable(false)
        }),
        Container::new(Label::new("Open it").style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::link())
                .selectable(false)
        }))
        .debug_name("Changescape open page")
        .on_event_stop(el::Click, move |_, _| open_html(state))
        .style(|s| clickable(underlined_link(s))),
    ))
    .style(|s| s.items_center().gap(8.0))
}

/// Open the page Changescape wrote for a browser.
fn open_html(state: ReviewState) {
    let Some(path) =
        shown(state).and_then(|shown| shown.lookup().drawable().map(|build| build.html_path()))
    else {
        return;
    };
    if let Err(e) = std::process::Command::new("open").arg(&path).spawn() {
        tracing::warn!(path = %path.display(), error = %e, "could not open the changescape page");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backticks_become_code_spans_and_offsets_follow() {
        let (plain, code, map) = backticks("a `b` c");
        assert_eq!(plain, "a b c");
        assert_eq!(code, vec![2..3_usize]);
        assert_eq!(map(6), 4, "the c");
        let (plain, code, _) = backticks("one ` alone");
        assert_eq!(plain, "one ` alone");
        assert!(code.is_empty());
    }

    #[test]
    fn a_short_column_keeps_its_width_and_a_long_one_starts_wider() {
        let table: Table = serde_json::from_value(serde_json::json!({
            "columns": [
                {"key": "sha", "label": "Commit", "kind": "mono"},
                {"key": "msg", "label": "Message", "kind": "text"},
                {"key": "files", "label": "Files", "kind": "num"},
                {"key": "refs", "label": "Where", "kind": "refs"},
            ],
            "rows": [
                {"cells": {
                    "sha": "e71c42c",
                    "msg": "feat(check): partial composite key findings for fanout groups",
                    "files": 6,
                    "refs": ["test_fanout_key_groups_by_owner.py", "run.py"],
                }},
            ],
        }))
        .unwrap();
        let widths = column_widths(&table);
        let Width::Fixed(sha) = widths[0] else {
            panic!("a hash is snug: {:?}", widths[0]);
        };
        let Width::Content(msg) = widths[1] else {
            panic!("a message takes what it needs: {:?}", widths[1]);
        };
        assert!(sha < SNUG && msg > 2.0 * sha, "{sha} {msg}");
        assert_eq!(widths[2], Width::Fixed(64.0));
        let Width::Content(refs) = widths[3] else {
            panic!("a refs column sizes by its widest entry: {:?}", widths[3]);
        };
        let one = cell_text_width("test_fanout_key_groups_by_owner.py", true);
        assert!(refs > one && refs < one + 40.0, "{refs} {one}");
    }
}
