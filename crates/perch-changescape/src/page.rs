//! `page.json`, read into the types generated from the page format's
//! schema (`v3`), and Perch's helpers over them. The contract is
//! `skill/docs/page-data.md` in the Changescape repository: the schema
//! gives the shapes, and that file the meanings.
//!
//! The generated types are strict, so one bad block would fail the whole
//! page. A page is read in two steps instead: each view's blocks one by
//! one, a block that does not read kept as [`PageBlock::Unknown`] with its
//! type's name, and then the rest of the page.

use std::collections::BTreeMap;
use std::ops::Deref;

use serde_json::Value;

use crate::v3;

/// A page this Perch draws: the generated page, with its views' blocks
/// read one by one beside it.
#[derive(Debug, Clone)]
pub struct LoadedPage {
    page: v3::Page,
    /// The blocks of each view, in view order.
    blocks: Vec<Vec<PageBlock>>,
}

/// One block of a view: read, or of a type or shape this Perch does not
/// know, which the browser page draws.
#[derive(Debug, Clone)]
pub enum PageBlock {
    Known(v3::Block),
    Unknown { kind: String },
}

/// Why a page was not read.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    /// The page is of a format version this Perch does not draw.
    #[error("page format {0}, which this Perch does not draw")]
    Version(u32),
}

/// The format version a page names: its `schema`, else 2.
pub fn schema_of(value: &Value) -> u32 {
    value
        .get("schema")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(crate::UNSTATED_PAGE_SCHEMA)
}

impl LoadedPage {
    /// Read a page: its version first, which must be one this Perch
    /// draws, then each view's blocks on their own, then the rest.
    pub fn read(bytes: &[u8]) -> Result<LoadedPage, ReadError> {
        let mut value: Value = serde_json::from_slice(bytes)?;
        let schema = schema_of(&value);
        if crate::Version::of(schema) != crate::Version::Drawn {
            return Err(ReadError::Version(schema));
        }
        let mut blocks = Vec::new();
        if let Some(views) = value.get_mut("views").and_then(Value::as_array_mut) {
            for view in views {
                let taken = match view.get_mut("blocks").map(Value::take) {
                    Some(Value::Array(taken)) => taken,
                    _ => Vec::new(),
                };
                if let Some(slot) = view.get_mut("blocks") {
                    *slot = Value::Array(Vec::new());
                }
                blocks.push(taken.into_iter().map(PageBlock::read).collect());
            }
        }
        let page: v3::Page = serde_json::from_value(value)?;
        Ok(LoadedPage { page, blocks })
    }

    /// The blocks of the view `id`.
    pub fn blocks(&self, id: &str) -> &[PageBlock] {
        self.page
            .views
            .iter()
            .position(|view| view.id == id)
            .and_then(|i| self.blocks.get(i))
            .map_or(&[], Vec::as_slice)
    }

    /// The view `id`.
    pub fn view(&self, id: &str) -> Option<&v3::View> {
        self.page.views.iter().find(|view| view.id == id)
    }

    /// A section's views, in order.
    pub fn section_views<'a>(&'a self, section: &'a v3::Section) -> Vec<&'a v3::View> {
        section
            .views
            .iter()
            .filter_map(|id| self.view(id))
            .collect()
    }

    /// The section a view is in.
    pub fn section_of_view(&self, view: &str) -> Option<&v3::Section> {
        self.page
            .sections
            .iter()
            .find(|section| section.views.iter().any(|id| id == view))
    }

    /// The first section, in rail order, whose views show the element.
    pub fn first_section_showing(&self, el: &str) -> Option<&v3::Section> {
        self.page.sections.iter().find(|section| {
            section.views.iter().any(|view| {
                self.blocks(view)
                    .iter()
                    .any(|block| block.elements().any(|shown| shown == el))
            })
        })
    }

    /// The option `option` of facet `facet`, if the page has it.
    pub fn facet_option(&self, facet: &str, option: &str) -> Option<&v3::FacetOption> {
        self.page
            .facets
            .iter()
            .find(|f| f.id == facet)?
            .options
            .iter()
            .find(|o| o.id == option)
    }
}

impl Deref for LoadedPage {
    type Target = v3::Page;

    fn deref(&self) -> &v3::Page {
        &self.page
    }
}

impl PageBlock {
    fn read(value: Value) -> PageBlock {
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match serde_json::from_value(value) {
            Ok(block) => PageBlock::Known(block),
            Err(_) => PageBlock::Unknown { kind },
        }
    }

    /// The `type` as written.
    pub fn kind(&self) -> &str {
        match self {
            PageBlock::Unknown { kind } => kind,
            PageBlock::Known(block) => match block {
                v3::Block::Narration { .. } => "narration",
                v3::Block::Text { .. } => "text",
                v3::Block::Summary { .. } => "summary",
                v3::Block::Table { .. } => "table",
                v3::Block::Svg { .. } => "svg",
                v3::Block::Lanes { .. } => "lanes",
                v3::Block::Graph { .. } => "graph",
                v3::Block::Code => "code",
                v3::Block::Discussion { .. } => "discussion",
            },
        }
    }

    /// The heading over the block.
    pub fn title(&self) -> Option<&str> {
        let PageBlock::Known(block) = self else {
            return None;
        };
        match block {
            v3::Block::Text { title, .. }
            | v3::Block::Summary { title, .. }
            | v3::Block::Table { title, .. }
            | v3::Block::Svg { title, .. }
            | v3::Block::Lanes { title, .. }
            | v3::Block::Graph { title, .. }
            | v3::Block::Discussion { title, .. } => title.as_deref(),
            v3::Block::Narration { .. } | v3::Block::Code => None,
        }
    }

    /// The muted line under the block.
    pub fn note(&self) -> Option<&str> {
        let PageBlock::Known(block) = self else {
            return None;
        };
        match block {
            v3::Block::Text { note, .. }
            | v3::Block::Table { note, .. }
            | v3::Block::Svg { note, .. }
            | v3::Block::Lanes { note, .. }
            | v3::Block::Graph { note, .. } => note.as_deref(),
            _ => None,
        }
    }

    /// The block as a table, when it is one.
    pub fn table(&self) -> Option<Table> {
        match self {
            PageBlock::Known(v3::Block::Table {
                columns,
                rows,
                groups,
                fold,
                ..
            }) => Some(Table {
                columns: columns.clone(),
                rows: rows.clone(),
                groups: groups.clone(),
                fold: fold.clone(),
            }),
            _ => None,
        }
    }

    /// The block as a diagram Perch draws, when it is one.
    pub fn svg(&self) -> Option<Svg> {
        match self {
            PageBlock::Known(v3::Block::Svg { svg, hot, .. }) => Some(Svg {
                svg: svg.clone(),
                hot: hot.iter().map(|(n, e)| (n.clone(), e.clone())).collect(),
            }),
            _ => None,
        }
    }

    /// Every element the block links to.
    pub fn elements(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        let PageBlock::Known(block) = self else {
            return Box::new(std::iter::empty());
        };
        match block {
            v3::Block::Table { rows, .. } => Box::new(rows.iter().flat_map(|row| {
                row.el
                    .as_deref()
                    .into_iter()
                    .chain(row.cells.values().flat_map(cell_elements))
            })),
            v3::Block::Svg { hot, .. } => Box::new(hot.values().map(String::as_str)),
            // A graph's node names an element and its status.
            v3::Block::Graph { hot, .. } => Box::new(hot.values().map(|label| label.0[0].as_str())),
            _ => Box::new(std::iter::empty()),
        }
    }
}

fn cell_elements(cell: &v3::Cell) -> Box<dyn Iterator<Item = &str> + '_> {
    match cell {
        v3::Cell::CellLink(link) => Box::new(link.el.as_deref().into_iter()),
        v3::Cell::Array(items) => Box::new(items.iter().filter_map(|item| match item {
            v3::CellListItem::CellLink(link) => link.el.as_deref(),
            v3::CellListItem::String(_) => None,
        })),
        _ => Box::new(std::iter::empty()),
    }
}

/// A table block's parts, owned, for a view that keeps them.
#[derive(Debug, Clone, Default)]
pub struct Table {
    pub columns: Vec<v3::Column>,
    pub rows: Vec<v3::Row>,
    pub groups: Vec<v3::TableGroup>,
    pub fold: Option<v3::Fold>,
}

/// How a column draws its cells: `text`, `mono`, `num`, `status`, `bar`
/// or `refs`. A column without a kind draws plain text.
pub fn column_kind(column: &v3::Column) -> &'static str {
    match column.kind {
        None | Some(v3::ColumnKind::Text) => "text",
        Some(v3::ColumnKind::Mono) => "mono",
        Some(v3::ColumnKind::Num) => "num",
        Some(v3::ColumnKind::Status) => "status",
        Some(v3::ColumnKind::Bar) => "bar",
        Some(v3::ColumnKind::Refs) => "refs",
    }
}

/// How much a row matters: a key row is one to read first, a minor one
/// folds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Rank {
    Key,
    #[default]
    Normal,
    Minor,
}

/// A row's rank. The schema leaves it an open string: anything but `key`
/// and `minor` is a normal row.
pub fn rank(row: &v3::Row) -> Rank {
    match row.rank.as_deref() {
        Some("key") => Rank::Key,
        Some("minor") => Rank::Minor,
        _ => Rank::Normal,
    }
}

/// A table cell as Perch draws it, whichever shape the file gives it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CellText {
    pub text: String,
    /// The number, for `num` and `bar` columns.
    pub value: Option<f64>,
    pub el: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    /// A list cell's entries, each its own cell.
    pub items: Vec<CellText>,
}

impl From<&v3::Cell> for CellText {
    fn from(cell: &v3::Cell) -> CellText {
        match cell {
            v3::Cell::String(text) => CellText {
                text: text.clone(),
                ..CellText::default()
            },
            v3::Cell::Number(n) => CellText {
                text: number_text(*n),
                value: Some(*n),
                ..CellText::default()
            },
            v3::Cell::CellLink(link) => CellText::from(link),
            // A list cell: its entries, and their texts joined.
            v3::Cell::Array(items) => {
                let items: Vec<CellText> = items
                    .iter()
                    .map(|item| match item {
                        v3::CellListItem::String(text) => CellText {
                            text: text.clone(),
                            ..CellText::default()
                        },
                        v3::CellListItem::CellLink(link) => CellText::from(link),
                    })
                    .collect();
                CellText {
                    text: items
                        .iter()
                        .map(|item| item.text.as_str())
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join(", "),
                    items,
                    ..CellText::default()
                }
            }
        }
    }
}

impl From<&v3::CellLink> for CellText {
    fn from(link: &v3::CellLink) -> CellText {
        CellText {
            text: link.text.clone(),
            value: link.text.trim().parse().ok(),
            el: link.el.clone(),
            file: link.file.clone(),
            line: link.line.and_then(|n| u32::try_from(n).ok()),
            items: Vec::new(),
        }
    }
}

/// A number as the file wrote it: `3`, not `3.0`.
fn number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

/// The cell of `row` under the column `key`, empty when the row has none.
pub fn cell(row: &v3::Row, key: &str) -> CellText {
    row.cells.get(key).map(CellText::from).unwrap_or_default()
}

/// A diagram Changescape laid out, as one SVG document. `hot` maps an SVG
/// node's `id` to the element it stands for.
#[derive(Debug, Clone, Default)]
pub struct Svg {
    pub svg: String,
    pub hot: BTreeMap<String, String>,
}

/// A line range as 1-based `(first, last)`.
pub fn lines(range: &Option<v3::LineRange>) -> Option<(u32, u32)> {
    let [first, last] = range.as_ref()?.0;
    Some((u32::try_from(first).ok()?, u32::try_from(last).ok()?))
}

/// One row of an element's diff: `[base_line, head_line, op, text]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SrcRow {
    pub base: Option<u32>,
    pub head: Option<u32>,
    pub op: String,
    pub text: String,
}

impl From<&v3::DiffLine> for SrcRow {
    fn from(line: &v3::DiffLine) -> SrcRow {
        let number = |v: &Value| v.as_u64().and_then(|n| u32::try_from(n).ok());
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        SrcRow {
            base: number(&line.0[0]),
            head: number(&line.0[1]),
            op: text(&line.0[2]),
            text: text(&line.0[3]),
        }
    }
}

/// A ref's own diff, row by row.
pub fn src_rows(r: &v3::ElementRef) -> Vec<SrcRow> {
    r.src.iter().map(SrcRow::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(json: serde_json::Value) -> v3::Row {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn cells_read_every_shape() {
        let r = row(serde_json::json!({ "cells": {
            "a": "x", "b": 3, "c": { "text": "f()", "el": "fn:f", "side": "head" },
            "d": { "text": "x.py:4", "file": "x.py", "line": 4 },
            "e": ["a.py", { "text": "b.py", "el": "mod:b" }] } }));
        assert_eq!(cell(&r, "a").text, "x");
        assert_eq!(cell(&r, "b").value, Some(3.0));
        assert_eq!(cell(&r, "b").text, "3");
        assert_eq!(cell(&r, "c").el.as_deref(), Some("fn:f"));
        assert_eq!(cell(&r, "d").line, Some(4));
        assert_eq!(cell(&r, "e").text, "a.py, b.py");
        assert_eq!(cell(&r, "e").items[1].el.as_deref(), Some("mod:b"));
        assert_eq!(cell(&r, "missing"), CellText::default());
    }

    #[test]
    fn a_rank_reads_key_and_minor_and_anything_else_as_normal() {
        let ranks: Vec<Rank> = ["key", "minor", "pinned"]
            .iter()
            .map(|rank| row(serde_json::json!({ "cells": {}, "rank": rank })))
            .chain([row(serde_json::json!({ "cells": {} }))])
            .map(|r| rank(&r))
            .collect();
        assert_eq!(ranks, [Rank::Key, Rank::Minor, Rank::Normal, Rank::Normal]);
    }

    #[test]
    fn a_diff_line_reads_as_a_src_row() {
        let line: v3::DiffLine = serde_json::from_str(r#"[10, null, "-", "def f():"]"#).unwrap();
        assert_eq!(
            SrcRow::from(&line),
            SrcRow {
                base: Some(10),
                head: None,
                op: "-".into(),
                text: "def f():".into()
            }
        );
    }
}
