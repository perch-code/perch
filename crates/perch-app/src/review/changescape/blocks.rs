//! A view on the Changescape tab: its title, its caption, and its blocks.
//! Text, summaries and tables are drawn here; diagrams in `svg`. A block of
//! a type Perch does not draw is one quiet line that offers the browser
//! page, which draws every type.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, RwSignal};
use floem::text::{Attrs, AttrsList, FamilyOwned};
use floem::views::{dyn_stack, Container, Decorators, Empty, Label, Stack, TextInput};
use floem::AnyView;
use perch_changescape::fold;
use perch_changescape::page::{cell, column_kind, rank, CellText as Cell, PageBlock, Rank, Table};
use perch_changescape::v3::{Block, Column, Row, SummaryItem, TableGroup as Group, View};
use perch_changescape::LoadedPage;

use crate::backend::Backend;
use crate::review::cards::tip_with;
use crate::review::changescape::order::{self, Sort};
use crate::review::changescape::{navigate, show_view, shown};
use crate::review::linked_text::linked_text;
use crate::review::model::{ChangescapeFoldKey, ReviewState};
use crate::theme;
use crate::ui::widgets::{clickable, underlined_link};

/// What every block needs to open what it names.
#[derive(Clone)]
pub struct Ctx {
    pub state: ReviewState,
    pub backend: Rc<Backend>,
    pub page: Arc<LoadedPage>,
    /// The open review threads on each element.
    pub counts: floem::reactive::Memo<Arc<super::threads::Counts>>,
    /// The elements whose rows never fold: the one the reader selected,
    /// and the one the Code strip names.
    pub pinned: Memo<Vec<String>>,
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

    pub fn open(&self, el: &str) {
        navigate::open_element(self.state, &self.backend, &self.page, el);
    }
}

/// A view, with its blocks as the page read them.
pub fn view(cx: Ctx, view: View, blocks: Vec<PageBlock>, titled: bool) -> impl IntoView {
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
    match view.narration.clone().filter(|c| !c.is_empty()) {
        Some(caption) => parts.push(paragraph(&caption).into_any()),
        // Where the caption goes, while the build is still writing it.
        None => {
            let state = cx.state;
            parts.push(
                Label::new("Caption coming…")
                    .debug_name("Changescape caption pending")
                    .style(move |s| {
                        let pending = shown(state).is_some_and(|shown| shown.captions_pending());
                        s.font_size(theme::TEXT_BODY)
                            .color(theme::fg_subtle())
                            .apply_if(!pending, |s| s.hide())
                    })
                    .into_any(),
            );
        }
    }
    for (i, block) in blocks.into_iter().enumerate() {
        parts.push(block_view(cx.clone(), block, (view.id.clone(), i)).into_any());
    }
    Stack::vertical_from_iter(parts)
        .debug_name(format!("Changescape view {}", view.id))
        .style(|s| s.flex_col().gap(8.0).width_full())
}

/// `at` is the block's view id and its index there, which key the state
/// of a table's sections.
fn block_view(cx: Ctx, block: PageBlock, at: (String, usize)) -> impl IntoView {
    let kind = block.kind().to_string();
    let body = match &block {
        PageBlock::Known(Block::Narration { text } | Block::Text { text, .. }) => {
            paragraph(text).into_any()
        }
        PageBlock::Known(Block::Summary { items, .. }) => summary_block(&cx, items).into_any(),
        PageBlock::Known(Block::Table { .. }) => {
            let table = block.table().unwrap_or_default();
            table_block(&cx, &table, at).into_any()
        }
        PageBlock::Known(Block::Svg { .. }) => {
            super::svg::svg_block(cx.clone(), block.svg().unwrap_or_default())
        }
        // Lanes, Mermaid graphs, the code view and the discussion are in
        // the browser page, as is any block this Perch could not read.
        PageBlock::Known(_) | PageBlock::Unknown { .. } => fallback(cx.state).into_any(),
    };
    let mut parts: Vec<AnyView> = Vec::new();
    if let Some(title) = block.title().map(str::to_string) {
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
    if let Some(note) = block.note().map(str::to_string) {
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

/// Whether Perch draws a block itself, not the browser page's line.
pub fn drawn(block: &PageBlock) -> bool {
    matches!(
        block,
        PageBlock::Known(
            Block::Narration { .. }
                | Block::Text { .. }
                | Block::Summary { .. }
                | Block::Table { .. }
                | Block::Svg { .. }
        )
    )
}

/// Prose with `code` spans in backticks.
fn paragraph(text: &str) -> impl IntoView {
    let (plain, code, _) = backticks(text);
    let len = plain.len();
    linked_text(plain, Vec::new(), move || {
        (prose_attrs(len, &code, &[]), Vec::new())
    })
    .style(|s| s.width_full())
}

/// The text without its backticks, the byte ranges that were in them, and
/// a map from a byte offset in `text` to one in the result.
fn backticks(text: &str) -> (String, Vec<Range<usize>>, impl Fn(usize) -> usize) {
    let mut plain = String::with_capacity(text.len());
    let mut code = Vec::new();
    let mut offsets = Vec::with_capacity(text.len() + 1);
    // Where the open backtick goes in the result, and where it is in `text`.
    let mut open: Option<(usize, usize)> = None;
    for (i, ch) in text.char_indices() {
        while offsets.len() <= i {
            offsets.push(plain.len());
        }
        if ch == '`' {
            match open.take() {
                Some((start, _)) => code.push(start..plain.len()),
                None => open = Some((plain.len(), i)),
            }
            continue;
        }
        plain.push(ch);
    }
    while offsets.len() <= text.len() {
        offsets.push(plain.len());
    }
    // An unclosed backtick is a backtick: what follows it moves over one.
    if let Some((start, tick)) = open {
        plain.insert(start, '`');
        for offset in offsets.iter_mut().skip(tick + 1) {
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
                .font_size(theme::PROSE_CODE_FONT_SIZE)
                .weight(theme::weight_body())
                .color(if linked { theme::link() } else { theme::fg() }),
        );
    }
    list
}

/// The overview's rows: a title, a metric, and a line, each opening its
/// view's section.
fn summary_block(cx: &Ctx, items: &[SummaryItem]) -> impl IntoView {
    let rows: Vec<AnyView> = items
        .iter()
        .map(|item| {
            let (state, page) = (cx.state, cx.page.clone());
            let view = item.view.clone();
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
            .on_event_stop(el::Click, move |_, _| show_view(state, &page, &view))
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
    /// Exactly this wide.
    Fixed(f64),
    /// Starts at its content's width, and gives way in proportion to it
    /// when the row runs short, down to `MIN_TEXT`.
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

/// Each column's width: a fixed kind's, or its widest text's, the
/// header's among them.
fn column_widths(table: &Table) -> Vec<Width> {
    table
        .columns
        .iter()
        .map(|column| {
            let kind = column_kind(column);
            if let Some(width) = column_width(kind) {
                return Width::Fixed(width);
            }
            let mono = is_mono(kind);
            let widest = table
                .rows
                .iter()
                .map(|row| cell(row, &column.key))
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

/// A column of names or paths, set in the code font.
fn is_mono(kind: &str) -> bool {
    kind == "mono" || kind == "refs"
}

/// With room for the arrow a sort puts after it.
fn header_width(label: &str) -> f64 {
    measure(
        &format!("{label} ↓"),
        theme::FONT_STACK,
        theme::TEXT_SECTION,
        true,
    )
}

fn cell_text_width(text: &str, mono: bool) -> f64 {
    if mono {
        measure(
            text,
            theme::CODE_FONT_STACK,
            theme::PROSE_CODE_FONT_SIZE,
            false,
        )
    } else {
        measure(text, theme::FONT_STACK, theme::TEXT_META, false)
    }
}

fn measure(text: &str, stack: &str, size: f32, emphasis: bool) -> f64 {
    let weight = emphasis.then(theme::weight_emphasis);
    crate::review::markdown::text_width_in(text, stack, size, weight)
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

/// A table this long or longer has a filter over it.
const FILTER_FROM: usize = 15;

/// One line of a table under its header.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Line {
    /// A section's header: its group, and whether it is open.
    Header { group: usize, open: bool },
    /// A row by index, dimmed or not.
    Row { row: usize, dim: bool },
    /// A part's control for its folded rows: what it says, and the part.
    More { group: Option<usize>, text: String },
}

/// The key of a part's state in `ReviewState`.
fn fold_key(at: &(String, usize), table: &Table, group: Option<usize>) -> ChangescapeFoldKey {
    let id = group
        .and_then(|g| table.groups.get(g))
        .map(|g| g.id.clone())
        .unwrap_or_default();
    (at.0.clone(), at.1, id)
}

/// The lines of a table as the page groups and folds it, or, sorted or
/// filtered, the matching rows alone. Tracked: the facet, the pinned
/// elements, and the state of the table's parts.
fn lines(
    cx: &Ctx,
    table: &Table,
    at: &(String, usize),
    visible: &[usize],
    arranged: bool,
) -> Vec<Line> {
    let state = cx.state;
    let hide = state.changescape_facet_hide.get();
    let pinned = cx.pinned.get();
    super::with_facet(state, &cx.page, |option| {
        let outside = |row: &Row| fold::outside(option, row);
        let keep = |row: &Row| !(hide && outside(row));
        let line = |row: usize| Line::Row {
            row,
            dim: outside(&table.rows[row]),
        };
        if arranged {
            return visible
                .iter()
                .copied()
                .filter(|&i| keep(&table.rows[i]))
                .map(line)
                .collect();
        }
        let pin = |row: &Row| {
            row.el.as_ref().is_some_and(|el| {
                pinned.contains(el) || option.is_some_and(|option| option.els.contains(el))
            })
        };
        let opened = state.changescape_open.get();
        let unfolded = state.changescape_unfolded.get();
        let mut lines = Vec::new();
        for part in fold::parts(table, keep, pin) {
            let key = fold_key(at, table, part.group);
            if let Some(group) = part.group {
                let open = part.pinned
                    || opened
                        .get(&key)
                        .copied()
                        .unwrap_or(table.groups[group].open);
                lines.push(Line::Header { group, open });
                if !open {
                    continue;
                }
            }
            if unfolded.contains(&key) {
                let mut rows: Vec<usize> = part.shown.iter().chain(&part.folded).copied().collect();
                rows.sort_unstable();
                lines.extend(rows.into_iter().map(line));
                if !part.folded.is_empty() {
                    lines.push(Line::More {
                        group: part.group,
                        text: "Show fewer".into(),
                    });
                }
            } else {
                lines.extend(part.shown.iter().copied().map(line));
                if !part.folded.is_empty() {
                    lines.push(Line::More {
                        group: part.group,
                        text: fold::more_label(&part, table.fold.as_ref()),
                    });
                }
            }
        }
        lines
    })
}

/// A table: its header, which sorts it by a column on a click, and its
/// rows, under a filter when there are many. Unsorted and unfiltered, the
/// rows are in the page's sections and folds; sorted or filtered, every
/// matching row shows in one list.
fn table_block(cx: &Ctx, table: &Table, at: (String, usize)) -> impl IntoView {
    let table = Rc::new(table.clone());
    let columns = Rc::new(table.columns.clone());
    // Over every row, folded ones too, so a part that opens moves no
    // column.
    let widths = Rc::new(column_widths(&table));
    // The longest bar in a column is full.
    let maxima: Rc<Vec<f64>> = Rc::new(
        columns
            .iter()
            .map(|column| {
                table
                    .rows
                    .iter()
                    .filter_map(|row| cell(row, &column.key).value)
                    .fold(0.0_f64, f64::max)
            })
            .collect(),
    );
    let badges = table.rows.iter().any(|row| row.el.is_some());
    let sort: RwSignal<Option<Sort>> = RwSignal::new(None);
    let query = RwSignal::new(String::new());
    let visible = {
        let table = table.clone();
        Memo::new(move |_| order::order(&table, sort.get(), &query.get()))
    };
    let shown_lines = {
        let (cx, table, at) = (cx.clone(), table.clone(), at.clone());
        Memo::new(move |_| {
            let arranged = sort.get().is_some() || query.with(|q| !q.trim().is_empty());
            visible.with(|visible| lines(&cx, &table, &at, visible, arranged))
        })
    };
    let header = Stack::horizontal_from_iter(
        columns
            .iter()
            .enumerate()
            .zip(widths.iter().copied())
            .map(|((i, column), width)| {
                let right = column_kind(column) == "num";
                let numeric = order::numeric(&table, &column.key);
                let label = column.label.clone();
                let text = move || match sort.get() {
                    Some(s) if s.column == i => {
                        format!("{label} {}", if s.descending { "↓" } else { "↑" })
                    }
                    _ => label.clone(),
                };
                Container::new(Label::derived(text).style(|s| {
                    s.font_size(theme::TEXT_SECTION)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg_muted())
                        .selectable(false)
                }))
                .debug_name(format!("Changescape sort {}", column.label))
                .on_event_stop(el::Click, move |_, _| {
                    sort.update(|sort| *sort = order::next(*sort, i, numeric))
                })
                .style(move |s| {
                    clickable(column_style(s, width))
                        .apply_if(right, |s| s.justify_end())
                        .hover(|s| s.color(theme::fg()))
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
        // Past the 2 a row keeps at its left for the bar of a key row.
        s.width_full()
            .padding_left(2.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(theme::border_muted())
    });
    let rows = {
        let (cx, table) = (cx.clone(), table.clone());
        dyn_stack(
            move || shown_lines.get(),
            |line: &Line| line.clone(),
            move |line| match line {
                Line::Row { row, dim } => table_row(
                    &cx,
                    &columns,
                    &widths,
                    &maxima,
                    &table.rows[row],
                    badges,
                    dim,
                )
                .into_any(),
                Line::Header { group, open } => {
                    let key = fold_key(&at, &table, Some(group));
                    section_header(cx.state, &table.groups[group], key, open).into_any()
                }
                Line::More { group, text } => {
                    let key = fold_key(&at, &table, group);
                    more_control(cx.state, key, text).into_any()
                }
            },
        )
        .style(|s| s.flex_col().width_full())
    };
    let total = table.rows.len();
    let filter = (total >= FILTER_FROM).then(|| filter_box(query, visible, total).into_any());
    let none = Label::new("No rows match the filter.").style(move |s| {
        s.font_size(theme::TEXT_META)
            .color(theme::fg_subtle())
            .padding_vert(8.0)
            .padding_horiz(CELL_PAD)
            .apply_if(!visible.with(Vec::is_empty), |s| s.hide())
    });
    Stack::vertical_from_iter(filter.into_iter().chain([
        header.into_any(),
        rows.into_any(),
        none.into_any(),
    ]))
    .style(|s| s.flex_col().width_full())
}

/// The box that keeps a long table to the rows with what is typed, and
/// how many those are.
fn filter_box(query: RwSignal<String>, visible: Memo<Vec<usize>>, total: usize) -> impl IntoView {
    let count = move || {
        if query.with(|q| q.trim().is_empty()) {
            String::new()
        } else {
            format!("{} of {total}", visible.with(Vec::len))
        }
    };
    Stack::horizontal((
        TextInput::new(query)
            .placeholder(format!("Filter {total} rows"))
            .debug_name("Changescape filter")
            .style(|s| {
                s.width(240.0)
                    .min_width(0.0)
                    .font_size(theme::TEXT_META)
                    .padding_horiz(8.0)
                    .padding_vert(4.0)
                    .border(1.0)
                    .border_color(theme::border_muted())
                    .border_radius(theme::RADIUS)
            }),
        Label::derived(count).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .margin_left(8.0)
        }),
    ))
    .style(|s| s.items_center().margin_bottom(4.0))
}

/// A section's header: a chevron, its label and its summary. A click
/// opens or closes it.
fn section_header(
    state: ReviewState,
    group: &Group,
    key: ChangescapeFoldKey,
    open: bool,
) -> impl IntoView {
    let summary = group
        .summary
        .as_deref()
        .map(|summary| backticks(summary).0)
        .unwrap_or_default();
    Stack::horizontal((
        Label::new(if open { "▾" } else { "▸" }).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .width(14.0)
                .flex_shrink(0.0_f32)
        }),
        Label::new(group.label.clone()).style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_strong())
                .color(theme::fg())
                .flex_shrink(0.0_f32)
        }),
        Label::new(summary).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .margin_left(10.0)
                .min_width(0.0)
                .text_ellipsis()
        }),
    ))
    .debug_name(format!("Changescape group {}", group.id))
    .on_event_stop(el::Click, move |_, _| {
        state.changescape_open.update(|opened| {
            opened.insert(key.clone(), !open);
        })
    })
    .style(|s| {
        clickable(
            s.width_full()
                .items_center()
                .padding_horiz(CELL_PAD)
                .padding_top(12.0)
                .padding_bottom(6.0)
                .border_bottom(1.0)
                .border_color(theme::border_muted())
                .selectable(false),
        )
        .hover(|s| s.background(theme::canvas_hover()))
    })
}

/// A part's control for its folded rows: it shows them in place, and then
/// folds them again.
fn more_control(state: ReviewState, key: ChangescapeFoldKey, text: String) -> impl IntoView {
    let name = format!("Changescape fold: {text}");
    Container::new(Label::new(text).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::link())
            .selectable(false)
    }))
    .debug_name(name)
    .on_event_stop(el::Click, move |_, _| {
        state.changescape_unfolded.update(|unfolded| {
            if !unfolded.remove(&key) {
                unfolded.insert(key.clone());
            }
        })
    })
    .style(|s| {
        clickable(
            s.width_full()
                .padding_horiz(CELL_PAD)
                .padding_vert(6.0)
                .border_bottom(1.0)
                .border_color(theme::border_muted()),
        )
        .hover(|s| s.background(theme::canvas_hover()))
    })
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
    outside: bool,
) -> impl IntoView {
    // Dimmed by the page, or outside the facet option the tab applies.
    let dim = row.dim == Some(true) || outside;
    let key = rank(row) == Rank::Key;
    let mut cells: Vec<AnyView> = columns
        .iter()
        .enumerate()
        .map(|(i, column)| {
            let cell = cell(row, &column.key);
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
                .style(move |s| {
                    clickable(row_style(s, key)).hover(|s| s.background(theme::canvas_hover()))
                })
                .into_any()
        }
        None => line.style(move |s| row_style(s, key)).into_any(),
    };
    let line: AnyView = match row.why.clone() {
        Some(why) => tip_with(line, move || why.clone()).into_any(),
        None => line,
    };
    Container::new(line)
        .debug_name(format!(
            "Changescape row{}{} {}",
            if key { " key" } else { "" },
            if dim { " dim" } else { "" },
            row.el.as_deref().unwrap_or("")
        ))
        .style(|s| s.width_full())
}

/// A row, with a bar in the accent at its left edge when it is a key row.
fn row_style(s: floem::style::Style, key: bool) -> floem::style::Style {
    let s = s
        .width_full()
        .items_center()
        .min_height(theme::SUMMARY_ROW)
        .border_bottom(1.0)
        .border_color(theme::border_muted());
    if key {
        s.border_left(2.0)
            .set(floem::style::BorderLeftColor, Some(theme::accent().into()))
    } else {
        s.border_left(2.0).set(
            floem::style::BorderLeftColor,
            Some(floem::peniko::Color::TRANSPARENT.into()),
        )
    }
}

fn table_cell(cx: &Ctx, column: &Column, width: Width, max: f64, cell: Cell, dim: bool) -> AnyView {
    let kind = column_kind(column);
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
    let body: AnyView = match kind {
        _ if !cell.items.is_empty() => {
            let links: Vec<AnyView> = cell
                .items
                .iter()
                .map(|item| {
                    let one = Column {
                        kind: Some(perch_changescape::v3::ColumnKind::Mono),
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
            let mono = is_mono(kind);
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
                        theme::PROSE_CODE_FONT_SIZE
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
                        .style(move |s| link_style(s, cut))
                        .into_any()
                }
                (None, Some(file)) => {
                    let (state, backend, line) = (cx.state, cx.backend.clone(), cell.line);
                    Container::new(label)
                        .on_event_stop(el::Click, move |_, _| {
                            navigate::open_file_line(state, &backend, &file, line)
                        })
                        .style(move |s| link_style(s, cut))
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

fn link_style(s: floem::style::Style, cut: bool) -> floem::style::Style {
    underlined_link(s)
        .min_width(0.0)
        .max_width_full()
        .apply_if(!cut, |s| s.width_full())
}

/// A block Perch does not draw: one line, and the browser page.
fn fallback(state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        Label::new("This view is in the browser page.").style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_subtle())
                .selectable(false)
        }),
        browser_link(state, "Open it"),
    ))
    .style(|s| s.items_center().gap(8.0))
}

/// A link that opens the page Changescape wrote for a browser.
pub fn browser_link(state: ReviewState, text: &'static str) -> Container {
    Container::new(Label::new(text).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::link())
            .selectable(false)
    }))
    .debug_name("Changescape open page")
    .on_event_stop(el::Click, move |_, _| open_html(state))
    .style(|s| clickable(underlined_link(s)))
}

/// Open the page Changescape wrote for a browser: the drawn page's, or
/// the one of a version this Perch refuses.
fn open_html(state: ReviewState) {
    let Some(path) = shown(state).and_then(|shown| {
        let lookup = shown.lookup();
        lookup
            .drawable()
            .or_else(|| lookup.refused())
            .map(|build| build.html_path())
    }) else {
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
        let (plain, code, map) = backticks("one ` alone");
        assert_eq!(plain, "one ` alone");
        assert!(code.is_empty());
        assert_eq!(map(4), 4, "the backtick");
        assert_eq!(map(6), 6, "the a after it");
    }

    fn table(columns: serde_json::Value, cells: serde_json::Value) -> Table {
        Table {
            columns: serde_json::from_value(columns).unwrap(),
            rows: serde_json::from_value(serde_json::json!([{"cells": cells}])).unwrap(),
            ..Table::default()
        }
    }

    /// The width a text column asks for: its text and the room each side.
    fn padded(width: f64) -> f64 {
        width.ceil() + 2.0 * CELL_PAD + 2.0
    }

    #[test]
    fn a_column_is_as_wide_as_its_widest_text_up_to_a_cap() {
        let message = "feat(check): partial composite key findings";
        let path = "test_fanout_key_groups_by_owner.py";
        let widths = column_widths(&table(
            serde_json::json!([
                {"key": "sha", "label": "Commit", "kind": "mono"},
                {"key": "msg", "label": "Message", "kind": "text"},
                {"key": "refs", "label": "Where", "kind": "refs"},
                {"key": "long", "label": "Long", "kind": "text"},
            ]),
            serde_json::json!({
                "sha": "e71c42c",
                "msg": message,
                "refs": [path, "run.py"],
                "long": message.repeat(4),
            }),
        ));
        assert_eq!(
            widths[0],
            Width::Fixed(padded(cell_text_width("e71c42c", true)))
        );
        assert_eq!(
            widths[1],
            Width::Content(padded(cell_text_width(message, false)))
        );
        // A refs column is as wide as its widest entry, not their join.
        assert_eq!(
            widths[2],
            Width::Content(padded(cell_text_width(path, true)))
        );
        assert_eq!(widths[3], Width::Content(MAX_TEXT));
    }

    #[test]
    fn a_header_wider_than_its_cells_sets_the_width() {
        let label = "Reached through, with its callers";
        let widths = column_widths(&table(
            serde_json::json!([{"key": "a", "label": label, "kind": "text"}]),
            serde_json::json!({"a": "x"}),
        ));
        assert_eq!(widths[0], Width::Content(padded(header_width(label))));
    }
}
