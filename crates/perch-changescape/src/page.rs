//! `page.json`: the facts and captions of one map, as Changescape writes
//! them. The contract is `skill/docs/page-data.md` in the Changescape
//! repository; these types read the part of it that Perch draws.
//!
//! Changescape grows faster than Perch, so reading never fails on what it
//! does not know: every struct ignores fields it has no place for, a list
//! drops an entry it cannot read rather than the whole page, and a block of
//! a type Perch does not draw is kept as [`BlockBody::Other`] with its name.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Page {
    pub meta: Meta,
    #[serde(deserialize_with = "lenient_vec")]
    pub sections: Vec<Section>,
    #[serde(deserialize_with = "lenient_map")]
    pub elements: BTreeMap<String, Element>,
    #[serde(deserialize_with = "lenient_vec")]
    pub views: Vec<View>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Meta {
    #[serde(deserialize_with = "text")]
    pub title: String,
}

/// One entry of the rail. A section with no views carries the `reason` it
/// has none, and the rail shows that line alone.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Section {
    #[serde(deserialize_with = "text")]
    pub id: String,
    /// The label, `label` in the file.
    #[serde(alias = "label", deserialize_with = "text")]
    pub t: String,
    /// The question the section answers.
    #[serde(deserialize_with = "text")]
    pub q: String,
    #[serde(deserialize_with = "text")]
    pub empty: String,
    #[serde(deserialize_with = "text")]
    pub reason: String,
    /// The section's views, in order, when the file lists them. Without
    /// the list, the views are the ones whose `sec` is this section, in
    /// file order.
    pub views: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct View {
    #[serde(deserialize_with = "text")]
    pub id: String,
    #[serde(deserialize_with = "text")]
    pub sec: String,
    /// The title.
    #[serde(deserialize_with = "text")]
    pub t: String,
    /// The subtitle.
    #[serde(deserialize_with = "text")]
    pub s: String,
    #[serde(deserialize_with = "text")]
    pub q: String,
    #[serde(deserialize_with = "text")]
    pub metric: String,
    /// The caption Changescape's narration wrote, if it ran.
    #[serde(deserialize_with = "lenient_option")]
    pub narration: Option<String>,
    #[serde(deserialize_with = "lenient_vec")]
    pub blocks: Vec<Block>,
}

/// Anything a view draws that maps to code or to a fact.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Element {
    /// The title.
    #[serde(deserialize_with = "text")]
    pub t: String,
    /// The kind, for example "function · dblect.sql.window_ties".
    #[serde(deserialize_with = "text")]
    pub k: String,
    /// One line about what changed.
    #[serde(deserialize_with = "text")]
    pub d: String,
    #[serde(deserialize_with = "lenient_vec")]
    pub refs: Vec<Ref>,
}

/// Where an element is: a file, and an inclusive, 1-based line range on
/// each side it is on.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Ref {
    #[serde(deserialize_with = "text")]
    pub f: String,
    #[serde(deserialize_with = "lenient_option")]
    pub base: Option<(u32, u32)>,
    #[serde(deserialize_with = "lenient_option")]
    pub head: Option<(u32, u32)>,
    /// The element's own unified diff.
    #[serde(deserialize_with = "lenient_vec")]
    pub src: Vec<SrcRow>,
    /// The element is the whole file, a module.
    pub file: bool,
}

/// One row of an element's diff: `[base_line, head_line, op, text]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SrcRow {
    pub base: Option<u32>,
    pub head: Option<u32>,
    pub op: String,
    pub text: String,
}

impl<'de> Deserialize<'de> for SrcRow {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (base, head, op, text) = <(Option<u32>, Option<u32>, String, String)>::deserialize(d)?;
        Ok(SrcRow {
            base,
            head,
            op,
            text,
        })
    }
}

/// One block of a view: what every block has, and what its type has.
#[derive(Debug, Clone)]
pub struct Block {
    /// The `type` as written, so a block Perch does not draw can still say
    /// what it is.
    pub kind: String,
    pub title: Option<String>,
    /// A muted line under the block.
    pub note: Option<String>,
    pub body: BlockBody,
}

#[derive(Debug, Clone)]
pub enum BlockBody {
    /// `narration` and `text`.
    Text(Text),
    Summary(Summary),
    Table(Table),
    Svg(Svg),
    /// A type Perch does not draw, or a known type it could not read.
    Other,
}

/// A paragraph, with `code` spans in backticks. An `el` makes all of it a
/// link; spans, when the file splits the text into them, can each carry
/// their own.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Text {
    #[serde(deserialize_with = "text")]
    pub text: String,
    pub el: Option<String>,
    #[serde(deserialize_with = "lenient_vec")]
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Span {
    #[serde(deserialize_with = "text")]
    pub text: String,
    pub el: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Summary {
    #[serde(deserialize_with = "lenient_vec")]
    pub items: Vec<SummaryItem>,
}

/// One row of a summary. It opens `view`, or `sec` when it names a section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SummaryItem {
    pub view: Option<String>,
    pub sec: Option<String>,
    #[serde(deserialize_with = "text")]
    pub title: String,
    #[serde(deserialize_with = "text")]
    pub metric: String,
    #[serde(deserialize_with = "text")]
    pub text: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Table {
    #[serde(deserialize_with = "lenient_vec")]
    pub columns: Vec<Column>,
    #[serde(deserialize_with = "lenient_vec")]
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Column {
    #[serde(deserialize_with = "text")]
    pub key: String,
    #[serde(deserialize_with = "text")]
    pub label: String,
    /// `text`, `mono`, `num`, `status`, `bar` or `refs`.
    #[serde(deserialize_with = "text")]
    pub kind: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Row {
    pub el: Option<String>,
    pub dim: bool,
    #[serde(deserialize_with = "lenient_map")]
    pub cells: BTreeMap<String, Cell>,
}

/// A table cell. The file writes a bare string or number, or an object
/// that can link to an element or to a file's line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cell {
    pub text: String,
    /// The number, for `num` and `bar` columns.
    pub value: Option<f64>,
    pub el: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    /// A `refs` cell's entries, each its own cell.
    pub items: Vec<Cell>,
}

impl<'de> Deserialize<'de> for Cell {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Cell::from_value(&Value::deserialize(d)?))
    }
}

impl Cell {
    fn from_value(value: &Value) -> Cell {
        match value {
            Value::String(text) => Cell {
                text: text.clone(),
                ..Cell::default()
            },
            Value::Number(n) => Cell {
                text: n.to_string(),
                value: n.as_f64(),
                ..Cell::default()
            },
            Value::Bool(b) => Cell {
                text: b.to_string(),
                ..Cell::default()
            },
            // A `refs` cell: its entries, and their texts joined.
            Value::Array(items) => {
                let items: Vec<Cell> = items.iter().map(Cell::from_value).collect();
                Cell {
                    text: items
                        .iter()
                        .map(|item| item.text.as_str())
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join(", "),
                    items,
                    ..Cell::default()
                }
            }
            Value::Object(map) => {
                let str_of = |key: &str| map.get(key).and_then(Value::as_str).map(str::to_string);
                let value = map
                    .get("value")
                    .or_else(|| map.get("text"))
                    .and_then(Value::as_f64);
                let text = match map.get("text") {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Number(n)) => n.to_string(),
                    _ => String::new(),
                };
                Cell {
                    text,
                    value,
                    el: str_of("el"),
                    file: str_of("file"),
                    line: map
                        .get("line")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok()),
                    items: Vec::new(),
                }
            }
            Value::Null => Cell::default(),
        }
    }
}

/// A diagram Changescape laid out, as one SVG document. `hot` maps an SVG
/// node's `id` to the element it stands for.
#[derive(Debug, Clone, Default)]
pub struct Svg {
    pub svg: String,
    pub hot: BTreeMap<String, String>,
}

impl<'de> Deserialize<'de> for Svg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            svg: String,
            #[serde(default)]
            hot: BTreeMap<String, Value>,
        }
        let raw = Raw::deserialize(d)?;
        // A value is the element id, or, as `graph` blocks write it,
        // `[element, status]`.
        let hot = raw
            .hot
            .into_iter()
            .filter_map(|(node, value)| {
                let el = match value {
                    Value::String(el) => el,
                    Value::Array(items) => items.first()?.as_str()?.to_string(),
                    _ => return None,
                };
                Some((node, el))
            })
            .collect();
        Ok(Svg { svg: raw.svg, hot })
    }
}

impl<'de> Deserialize<'de> for Block {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        let Value::Object(map) = &value else {
            return Err(serde::de::Error::custom("a block is not an object"));
        };
        let str_of = |key: &str| map.get(key).and_then(Value::as_str).map(str::to_string);
        let kind = str_of("type").unwrap_or_default();
        fn read<T: DeserializeOwned>(value: &Value, wrap: fn(T) -> BlockBody) -> BlockBody {
            serde_json::from_value(value.clone()).map_or(BlockBody::Other, wrap)
        }
        let body = match kind.as_str() {
            "narration" | "text" => read(&value, BlockBody::Text),
            "summary" => read(&value, BlockBody::Summary),
            "table" => read(&value, BlockBody::Table),
            "svg" => read(&value, BlockBody::Svg),
            _ => BlockBody::Other,
        };
        Ok(Block {
            kind,
            title: str_of("title"),
            note: str_of("note"),
            body,
        })
    }
}

impl Page {
    /// A section's views, in order.
    pub fn section_views<'a>(&'a self, section: &'a Section) -> Vec<&'a View> {
        match &section.views {
            Some(ids) => ids
                .iter()
                .filter_map(|id| self.views.iter().find(|view| &view.id == id))
                .collect(),
            None => self
                .views
                .iter()
                .filter(|view| view.sec == section.id)
                .collect(),
        }
    }

    /// The section a view is in.
    pub fn section_of_view(&self, view: &str) -> Option<&Section> {
        self.sections.iter().find(|section| {
            self.section_views(section)
                .iter()
                .any(|shown| shown.id == view)
        })
    }

    /// The first section, in rail order, whose views show the element.
    pub fn first_section_showing(&self, el: &str) -> Option<&Section> {
        self.sections.iter().find(|section| {
            self.section_views(section).iter().any(|view| {
                view.blocks
                    .iter()
                    .any(|block| block.elements().any(|shown| shown == el))
            })
        })
    }
}

impl Block {
    /// Every element the block links to.
    pub fn elements(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match &self.body {
            BlockBody::Text(text) => Box::new(
                text.el
                    .as_deref()
                    .into_iter()
                    .chain(text.spans.iter().filter_map(|span| span.el.as_deref())),
            ),
            BlockBody::Table(table) => Box::new(table.rows.iter().flat_map(|row| {
                row.el
                    .as_deref()
                    .into_iter()
                    .chain(row.cells.values().filter_map(|cell| cell.el.as_deref()))
            })),
            BlockBody::Svg(svg) => Box::new(svg.hot.values().map(String::as_str)),
            BlockBody::Summary(_) | BlockBody::Other => Box::new(std::iter::empty()),
        }
    }
}

/// A list that drops the entries it cannot read, and reads as empty when
/// it is not a list at all.
fn lenient_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(match Value::deserialize(d)? {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect(),
        _ => Vec::new(),
    })
}

/// A map that drops the entries it cannot read.
fn lenient_map<'de, D, T>(d: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(match Value::deserialize(d)? {
        Value::Object(map) => map
            .into_iter()
            .filter_map(|(key, item)| Some((key, serde_json::from_value(item).ok()?)))
            .collect(),
        _ => BTreeMap::new(),
    })
}

/// Text, empty when the file has null, a number, or nothing there.
fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(text) => text,
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}

/// A value that reads as `None` when it is null or not what it should be.
fn lenient_option<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(serde_json::from_value(Value::deserialize(d)?).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_block_type_is_kept_by_name() {
        let block: Block =
            serde_json::from_str(r#"{ "type": "lanes", "before": [], "title": "Lanes" }"#).unwrap();
        assert_eq!(block.kind, "lanes");
        assert_eq!(block.title.as_deref(), Some("Lanes"));
        assert!(matches!(block.body, BlockBody::Other));
    }

    #[test]
    fn a_known_block_it_cannot_read_falls_back() {
        let block: Block = serde_json::from_str(r#"{ "type": "svg", "svg": 12 }"#).unwrap();
        assert_eq!(block.kind, "svg");
        assert!(matches!(block.body, BlockBody::Other));
    }

    #[test]
    fn cells_read_every_shape() {
        let row: Row = serde_json::from_str(
            r#"{ "cells": { "a": "x", "b": 3, "c": { "text": "f()", "el": "fn:f", "side": "head" },
                 "d": { "text": "x.py:4", "file": "x.py", "line": 4 }, "e": ["a.py", "b.py"] } }"#,
        )
        .unwrap();
        assert_eq!(row.cells["a"].text, "x");
        assert_eq!(row.cells["b"].value, Some(3.0));
        assert_eq!(row.cells["c"].el.as_deref(), Some("fn:f"));
        assert_eq!(row.cells["d"].line, Some(4));
        assert_eq!(row.cells["e"].text, "a.py, b.py");
    }

    #[test]
    fn hot_reads_an_id_or_a_pair() {
        let svg: Svg = serde_json::from_str(
            r#"{ "svg": "<svg/>", "hot": { "n1": "fn:a", "n2": ["fn:b", "added"], "n3": 4 } }"#,
        )
        .unwrap();
        assert_eq!(svg.hot.len(), 2);
        assert_eq!(svg.hot["n2"], "fn:b");
    }

    #[test]
    fn a_section_takes_its_label_and_a_null_reason() {
        let section: Section = serde_json::from_str(
            r#"{ "id": "overview", "label": "Overview", "reason": null, "views": ["overview"] }"#,
        )
        .unwrap();
        assert_eq!(section.t, "Overview");
        assert_eq!(section.reason, "");
    }

    #[test]
    fn a_bad_ref_drops_alone() {
        let element: Element = serde_json::from_str(
            r#"{ "t": "f()", "refs": [ { "f": "a.py", "head": [1, 2] }, 7 ] }"#,
        )
        .unwrap();
        assert_eq!(element.refs.len(), 1);
        assert_eq!(element.refs[0].head, Some((1, 2)));
    }
}
