//! `page.json`, read into the types generated from the page format's
//! schema (`v3`), and Perch's helpers over them. The contract is
//! `skill/docs/page-data.md` in the Changescape repository: the schema
//! gives the shapes, and that file the meanings.
//!
//! Perch draws the page's tour, and only the tour. The generated types
//! are strict, so one bad stop would fail the whole page. A page is read
//! in two steps instead: the tour on its own, kept out when it does not
//! read, and then the rest. Perch draws no view as a section, so it does
//! not read the views' blocks, and a block of a new shape never fails a
//! page. It keeps only what a `view:<id>` ref shows: the element each
//! table row names, and each diagram.

use std::collections::{BTreeMap, HashMap};
use std::ops::Deref;
use std::path::Path;

use serde_json::Value;

use crate::{v3, Error};

/// A page this Perch draws: the generated page, with the tour read on its
/// own.
#[derive(Debug, Clone)]
pub struct LoadedPage {
    page: v3::Page,
    /// Why the page's tour was not read, when it has one that did not.
    tour_error: Option<String>,
    /// The elements each view's table rows name, by view id, in row
    /// order and each once.
    view_rows: HashMap<String, Vec<String>>,
    /// The diagrams of each view's `svg` blocks, by view id, in order.
    view_pictures: HashMap<String, Vec<Svg>>,
}

impl LoadedPage {
    /// Read a page: its tour on its own, then the rest without the views'
    /// blocks.
    pub(crate) fn read(path: &Path, bytes: &[u8]) -> Result<LoadedPage, Error> {
        let parse = |source| Error::Parse {
            path: path.to_path_buf(),
            source,
        };
        let mut value: Value = serde_json::from_slice(bytes).map_err(parse)?;
        let mut view_rows = HashMap::new();
        let mut view_pictures = HashMap::new();
        if let Some(views) = value.get_mut("views").and_then(Value::as_array_mut) {
            for view in views {
                let Some(blocks) = view.get_mut("blocks").map(Value::take) else {
                    continue;
                };
                view["blocks"] = Value::Array(Vec::new());
                if let Some(id) = view.get("id").and_then(Value::as_str) {
                    view_rows.insert(id.to_string(), row_elements(&blocks));
                    view_pictures.insert(id.to_string(), svg_blocks(&blocks));
                }
            }
        }
        let tour = value
            .as_object_mut()
            .and_then(|page| page.remove("tour"))
            .filter(|tour| !tour.is_null());
        let mut page: v3::Page = serde_json::from_value(value).map_err(parse)?;
        let mut tour_error = None;
        if let Some(tour) = tour {
            match serde_json::from_value::<v3::Tour>(tour) {
                Ok(tour) => page.tour = Some(tour),
                Err(e) => tour_error = Some(e.to_string()),
            }
        }
        Ok(LoadedPage {
            page,
            tour_error,
            view_rows,
            view_pictures,
        })
    }

    /// The view `id`.
    pub fn view(&self, id: &str) -> Option<&v3::View> {
        self.page.views.iter().find(|view| view.id == id)
    }

    /// The elements the view `id`'s table rows name, in row order, each
    /// once, leaving out any the page does not hold.
    pub fn view_rows(&self, id: &str) -> Vec<&str> {
        self.view_rows
            .get(id)
            .into_iter()
            .flatten()
            .filter(|el| self.page.elements.contains_key(*el))
            .map(String::as_str)
            .collect()
    }

    /// The diagrams of the view `id`, in block order.
    pub fn view_pictures(&self, id: &str) -> &[Svg] {
        self.view_pictures.get(id).map_or(&[], Vec::as_slice)
    }

    /// The tour, when the page has one that read.
    pub fn tour(&self) -> Option<&v3::Tour> {
        self.page.tour.as_ref()
    }

    /// Why the page's tour did not read.
    pub fn tour_error(&self) -> Option<&str> {
        self.tour_error.as_deref()
    }

    /// The stop `id` and the chapter it is in.
    pub fn stop(&self, id: &str) -> Option<(&v3::TourChapter, &v3::TourStop)> {
        self.tour()?.chapters.iter().find_map(|chapter| {
            chapter
                .stops
                .iter()
                .find(|stop| stop.id == id)
                .map(|stop| (chapter, stop))
        })
    }

    /// The comment a `comment:<kind>:<id>` ref names.
    pub fn comment(&self, r: &str) -> Option<&v3::Comment> {
        let (kind, id) = comment_ref(r)?;
        self.page
            .comments
            .iter()
            .find(|c| c.id == id && comment_kind(&c.kind) == kind)
    }
}

/// The `el` of every row of every block, in order and each once. Read
/// from the JSON as it is, so a block of any shape gives what it has.
fn row_elements(blocks: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let rows = blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("rows")?.as_array())
        .flatten();
    for el in rows.filter_map(|row| row.get("el")?.as_str()) {
        if !out.iter().any(|seen| seen == el) {
            out.push(el.to_string());
        }
    }
    out
}

/// Each `svg` block that has its SVG, with the `hot` entries that name
/// an element. Read from the JSON as it is, so a block of another shape
/// is left out, not an error.
fn svg_blocks(blocks: &Value) -> Vec<Svg> {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("svg"))
        .filter_map(|block| {
            let svg = block.get("svg")?.as_str().filter(|svg| !svg.is_empty())?;
            let hot = block
                .get("hot")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter_map(|(node, el)| Some((node.clone(), el.as_str()?.to_string())))
                .collect();
            Some(Svg {
                svg: svg.to_string(),
                hot,
            })
        })
        .collect()
}

/// What a ref names: a GitHub comment, a view, or an element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref<'a> {
    Comment(&'a str, i64),
    View(&'a str),
    Element(&'a str),
}

/// A ref as the contract writes it: `comment:<kind>:<id>`, `view:<id>`,
/// or an element id.
pub fn parse_ref(r: &str) -> Ref<'_> {
    if let Some((kind, id)) = comment_ref(r) {
        return Ref::Comment(kind, id);
    }
    match r.strip_prefix("view:") {
        Some(view) => Ref::View(view),
        None => Ref::Element(r),
    }
}

impl Deref for LoadedPage {
    type Target = v3::Page;

    fn deref(&self) -> &v3::Page {
        &self.page
    }
}

/// `value` if it is one of `known`, else `fallback`. The schema leaves
/// these sets open, and names the value an unknown one reads as.
fn known(value: &str, known: &[&'static str], fallback: &'static str) -> &'static str {
    known
        .iter()
        .find(|k| **k == value)
        .copied()
        .unwrap_or(fallback)
}

/// Where on GitHub a comment was left; a kind this Perch does not know is
/// `issue`.
fn comment_kind(kind: &v3::CommentKind) -> &'static str {
    known(kind, &["inline", "review", "issue"], "issue")
}

/// A `comment:<kind>:<id>` ref as its kind and GitHub id.
fn comment_ref(r: &str) -> Option<(&str, i64)> {
    let rest = r.strip_prefix("comment:")?;
    let (kind, id) = rest.split_once(':')?;
    Some((kind, id.parse().ok()?))
}

/// How closely to read a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The idea, or logic a reviewer must read: shown open.
    Read,
    /// Support code: folded into a row.
    Skim,
}

/// A stop's verdict: its own, else its chapter's, else read. The set is
/// open, and a value this Perch does not know reads.
pub fn verdict(chapter: &v3::TourChapter, stop: &v3::TourStop) -> Verdict {
    match stop.verdict.as_deref().or(chapter.verdict.as_deref()) {
        Some("skim") => Verdict::Skim,
        _ => Verdict::Read,
    }
}

/// One run of a chapter's stops as the tour shows them: a read stop on
/// its own, consecutive skim stops folded together, or the header of a
/// section, by its index in `sections`, before the section's first stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    Read(usize),
    Skim(Vec<usize>),
    Section(usize),
}

/// The section each stop of a chapter is in, by stop id: the index in
/// `sections`. A stop in two sections is in the first.
pub fn section_of(chapter: &v3::TourChapter) -> HashMap<&str, usize> {
    let mut out = HashMap::new();
    for (k, section) in chapter.sections.iter().enumerate() {
        for id in &section.stops {
            out.entry(id.as_str()).or_insert(k);
        }
    }
    out
}

/// A chapter's stops, by index, as runs, in the stops' order. A section's
/// header comes before its first stop, and a run of skim stops never
/// crosses one.
pub fn runs(chapter: &v3::TourChapter) -> Vec<Run> {
    let sections = section_of(chapter);
    let mut opened = Vec::new();
    let mut out: Vec<Run> = Vec::new();
    for (i, stop) in chapter.stops.iter().enumerate() {
        if let Some(&k) = sections.get(stop.id.as_str()) {
            if !opened.contains(&k) {
                opened.push(k);
                out.push(Run::Section(k));
            }
        }
        match (verdict(chapter, stop), out.last_mut()) {
            (Verdict::Read, _) => out.push(Run::Read(i)),
            (Verdict::Skim, Some(Run::Skim(group))) => group.push(i),
            (Verdict::Skim, _) => out.push(Run::Skim(vec![i])),
        }
    }
    out
}

/// Every stop of the tour in reading order, with its chapter.
pub fn stops(tour: &v3::Tour) -> impl Iterator<Item = (&v3::TourChapter, &v3::TourStop)> {
    tour.chapters
        .iter()
        .flat_map(|chapter| chapter.stops.iter().map(move |stop| (chapter, stop)))
}

/// The read stops of the tour, in reading order.
pub fn read_stops(tour: &v3::Tour) -> impl Iterator<Item = &v3::TourStop> {
    stops(tour)
        .filter(|(chapter, stop)| verdict(chapter, stop) == Verdict::Read)
        .map(|(_, stop)| stop)
}

/// The first read stop after the stop `id`, in reading order.
pub fn next_read(tour: &v3::Tour, id: &str) -> Option<String> {
    stops(tour)
        .skip_while(|(_, stop)| stop.id != id)
        .skip(1)
        .find(|(chapter, stop)| verdict(chapter, stop) == Verdict::Read)
        .map(|(_, stop)| stop.id.clone())
}

/// The kind of a picture this Perch draws. The set is open: a picture of
/// another kind is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureKind {
    Hunk,
    Seq,
    Other,
}

pub fn picture_kind(kind: &str) -> PictureKind {
    match kind {
        "hunk" => PictureKind::Hunk,
        "seq" => PictureKind::Seq,
        _ => PictureKind::Other,
    }
}

/// A window of a hunk picture: 1-based inclusive line ranges on each side,
/// either absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub base: Option<(u32, u32)>,
    pub head: Option<(u32, u32)>,
}

impl From<&v3::TourWindow> for Window {
    fn from(w: &v3::TourWindow) -> Window {
        Window {
            base: lines(&w.base),
            head: lines(&w.head),
        }
    }
}

/// A hunk picture's file and windows.
pub fn hunk(picture: &v3::TourPicture) -> Option<(&str, Vec<Window>)> {
    if picture_kind(&picture.kind) != PictureKind::Hunk {
        return None;
    }
    let file = picture.file.as_deref()?;
    Some((file, picture.windows.iter().map(Window::from).collect()))
}

/// A diagram Changescape laid out, as one SVG document. `hot` maps an SVG
/// node's `id` to the element it stands for.
#[derive(Debug, Clone, Default)]
pub struct Svg {
    pub svg: String,
    pub hot: BTreeMap<String, String>,
}

/// A seq picture's diagram.
pub fn picture_svg(picture: &v3::TourPicture) -> Option<Svg> {
    if picture_kind(&picture.kind) != PictureKind::Seq {
        return None;
    }
    Some(Svg {
        svg: picture.svg.clone()?,
        hot: picture.hot.clone().into_iter().collect(),
    })
}

/// A chapter's spine, as a diagram, when it is a seq.
pub fn spine_svg(spine: &v3::TourSpine) -> Option<Svg> {
    (picture_kind(&spine.kind) == PictureKind::Seq).then(|| Svg {
        svg: spine.svg.clone(),
        hot: spine.hot.clone().into_iter().collect(),
    })
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

    #[test]
    fn an_unknown_value_in_an_open_set_reads_as_the_schema_says() {
        assert_eq!(comment_kind(&v3::CommentKind("inline".into())), "inline");
        assert_eq!(comment_kind(&v3::CommentKind("commit".into())), "issue");
        assert_eq!(picture_kind("table"), PictureKind::Other);
    }

    #[test]
    fn a_ref_names_a_comment_a_view_or_an_element() {
        assert_eq!(comment_ref("comment:inline:42"), Some(("inline", 42)));
        assert_eq!(comment_ref("fn:a.b"), None);
        assert_eq!(comment_ref("comment:inline:x"), None);
        assert_eq!(parse_ref("comment:issue:7"), Ref::Comment("issue", 7));
        assert_eq!(parse_ref("view:related"), Ref::View("related"));
        assert_eq!(parse_ref("pr:2049"), Ref::Element("pr:2049"));
    }

    #[test]
    fn a_views_rows_are_the_elements_its_tables_name_each_once() {
        let blocks = serde_json::json!([
            { "type": "table", "rows": [{ "el": "pr:1" }, { "cells": {} }, { "el": "pr:2" }] },
            { "type": "text", "text": "x" },
            { "type": "heatmap", "rows": [{ "el": "pr:1" }, { "el": "pr:3" }] },
        ]);
        assert_eq!(row_elements(&blocks), ["pr:1", "pr:2", "pr:3"]);
    }

    #[test]
    fn a_views_pictures_are_its_svg_blocks_that_have_an_svg() {
        let blocks = serde_json::json!([
            { "type": "svg", "svg": "<svg/>", "hot": { "a": "job:x", "b": 7 } },
            { "type": "svg", "hot": { "a": "job:y" } },
            { "type": "svg", "svg": "" },
            { "type": "table", "svg": "<svg/>" },
            { "type": "svg", "svg": "<svg id='2'/>" },
        ]);
        let pictures = svg_blocks(&blocks);
        assert_eq!(pictures.len(), 2);
        assert_eq!(pictures[0].svg, "<svg/>");
        assert_eq!(
            pictures[0].hot,
            BTreeMap::from([("a".to_string(), "job:x".to_string())])
        );
        assert!(pictures[1].hot.is_empty());
    }

    fn chapter(verdicts: &[Option<&str>], own: Option<&str>) -> v3::TourChapter {
        let stops: Vec<serde_json::Value> = verdicts
            .iter()
            .enumerate()
            .map(|(i, v)| {
                serde_json::json!({ "id": format!("s{i}"), "title": "t", "els": [],
                    "pictures": [], "lines": 1, "verdict": v })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": "c", "title": "C", "verdict": own, "stops": stops }))
        .unwrap()
    }

    #[test]
    fn consecutive_skim_stops_fold_into_one_run() {
        let c = chapter(
            &[Some("read"), Some("skim"), Some("skim"), None, Some("skim")],
            None,
        );
        assert_eq!(
            runs(&c),
            [
                Run::Read(0),
                Run::Skim(vec![1, 2]),
                Run::Read(3),
                Run::Skim(vec![4])
            ]
        );
    }

    #[test]
    fn a_section_header_comes_before_its_first_stop_and_splits_a_skim_run() {
        let mut c = chapter(
            &[
                Some("read"),
                Some("skim"),
                Some("skim"),
                Some("read"),
                Some("read"),
            ],
            None,
        );
        c.sections = serde_json::from_value(serde_json::json!([
            { "title": "A", "stops": ["s0", "s1"] },
            { "title": "B", "stops": ["s2", "s3"] },
        ]))
        .unwrap();
        assert_eq!(
            runs(&c),
            [
                Run::Section(0),
                Run::Read(0),
                Run::Skim(vec![1]),
                Run::Section(1),
                Run::Skim(vec![2]),
                Run::Read(3),
                // In no section: after the last one.
                Run::Read(4),
            ]
        );
    }

    #[test]
    fn next_goes_to_the_next_read_stop_across_chapters() {
        let first = chapter(&[Some("read"), Some("skim")], None);
        let mut second = chapter(&[Some("skim"), Some("read")], None);
        for stop in &mut second.stops {
            stop.id = format!("t{}", stop.id);
        }
        let tour = v3::Tour {
            below: Vec::new(),
            chapters: vec![first, second],
            ending: None,
            flags: Vec::new(),
            map: Vec::new(),
            narrated: None,
            summary: None,
        };
        assert_eq!(next_read(&tour, "s0").as_deref(), Some("ts1"));
        assert_eq!(next_read(&tour, "s1").as_deref(), Some("ts1"));
        assert_eq!(next_read(&tour, "ts1"), None);
        let read: Vec<&str> = read_stops(&tour).map(|s| s.id.as_str()).collect();
        assert_eq!(read, ["s0", "ts1"]);
    }

    #[test]
    fn a_stop_without_a_verdict_takes_its_chapters_and_an_unknown_one_reads() {
        let c = chapter(&[None, Some("glance")], Some("skim"));
        assert_eq!(verdict(&c, &c.stops[0]), Verdict::Skim);
        assert_eq!(verdict(&c, &c.stops[1]), Verdict::Read);
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
