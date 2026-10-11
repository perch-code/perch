//! `page.json`, read into the types generated from the page format's
//! schema (`v4`), and Perch's helpers over them. The contract is
//! `skill/src/changescape/contract/` in the Changescape repository: the
//! schema gives the shapes, and `page-data.md` the meanings.
//!
//! Every field is present and every union is tagged, so a page reads
//! whole or not at all. Perch draws the tour, and a view only as the
//! popover a target opens: its diagrams and the elements its tables name.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::ops::Deref;
use std::path::Path;

use crate::v4::{self, BlocksItem, PicturesItem, Verdict, ViewId};
use crate::Error;

/// A page this Perch draws.
#[derive(Debug, Clone)]
pub struct LoadedPage {
    page: v4::Page,
}

impl LoadedPage {
    pub(crate) fn read(path: &Path, bytes: &[u8]) -> Result<LoadedPage, Error> {
        let page = serde_json::from_slice(bytes).map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(LoadedPage { page })
    }

    /// The view `id`, when the page holds it: only views that apply are
    /// on the page.
    pub fn view(&self, id: ViewId) -> Option<&v4::View> {
        self.page.views.iter().find(|view| view.id == id)
    }

    /// The elements the view `id`'s table rows name, in row order, each
    /// once, leaving out any the page does not hold.
    pub fn view_rows(&self, id: ViewId) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        let rows = self
            .view(id)
            .into_iter()
            .flat_map(|view| &view.blocks)
            .filter_map(|block| match block {
                BlocksItem::Table { rows, .. } => Some(rows),
                _ => None,
            })
            .flatten();
        for el in rows.filter_map(|row| row.element.as_deref()) {
            if self.page.elements.contains_key(el) && !out.contains(&el) {
                out.push(el);
            }
        }
        out
    }

    /// The diagrams of the view `id`, in block order.
    pub fn view_pictures(&self, id: ViewId) -> Vec<Svg> {
        self.view(id)
            .into_iter()
            .flat_map(|view| &view.blocks)
            .filter_map(|block| match block {
                BlocksItem::Svg { svg, hot, .. } => Some(Svg::new(svg, hot)),
                _ => None,
            })
            .collect()
    }

    /// The tour, when the page has one.
    pub fn tour(&self) -> Option<&v4::Tour> {
        self.page.tour.as_ref()
    }

    /// The stop `id` and the chapter it is in.
    pub fn stop(&self, id: &str) -> Option<(&v4::Chapter, &v4::Stop)> {
        self.tour()?.chapters.iter().find_map(|chapter| {
            chapter
                .stops
                .iter()
                .find(|stop| stop.id == id)
                .map(|stop| (chapter, stop))
        })
    }

    /// The comment a reference names: GitHub numbers comments of each
    /// kind on their own.
    pub fn comment(&self, r: &v4::CommentRef) -> Option<&v4::Comment> {
        self.page
            .comments
            .iter()
            .find(|c| c.id == r.id && c.kind == r.kind)
    }
}

impl Deref for LoadedPage {
    type Target = v4::Page;

    fn deref(&self) -> &v4::Page {
        &self.page
    }
}

/// What a flag or a stop's Watch line opens. The schema writes the two
/// targets as two unions of the same members.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target<'a> {
    Element(&'a str),
    View(ViewId),
    Comment(&'a v4::CommentRef),
}

impl<'a> From<&'a v4::FlagTarget> for Target<'a> {
    fn from(target: &'a v4::FlagTarget) -> Target<'a> {
        match target {
            v4::FlagTarget::Element { element } => Target::Element(element),
            v4::FlagTarget::View { view } => Target::View(*view),
            v4::FlagTarget::Comment { comment } => Target::Comment(comment),
        }
    }
}

impl<'a> From<&'a v4::StopWatchTarget> for Target<'a> {
    fn from(target: &'a v4::StopWatchTarget) -> Target<'a> {
        match target {
            v4::StopWatchTarget::Element { element } => Target::Element(element),
            v4::StopWatchTarget::View { view } => Target::View(*view),
            v4::StopWatchTarget::Comment { comment } => Target::Comment(comment),
        }
    }
}

/// Whether a stop folds into a row with its neighbours: support code to
/// skim, or code to skip, which the tour still lists.
pub fn folds(stop: &v4::Stop) -> bool {
    stop.verdict != Verdict::Read
}

/// One run of a chapter's stops as the tour shows them: a read stop on
/// its own, consecutive folded stops together, or the header of a
/// section, by its index in `sections`, before the section's first stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    Read(usize),
    Skim(Vec<usize>),
    Section(usize),
}

/// The section each stop of a chapter is in, by stop id: the index in
/// `sections`. A stop in two sections is in the first.
pub fn section_of(chapter: &v4::Chapter) -> HashMap<&str, usize> {
    let mut out = HashMap::new();
    for (k, section) in chapter.sections.iter().enumerate() {
        for id in &section.stops {
            out.entry(id.as_str()).or_insert(k);
        }
    }
    out
}

/// A chapter's stops, by index, as runs, in the stops' order. A section's
/// header comes before its first stop, and a run of folded stops never
/// crosses one.
pub fn runs(chapter: &v4::Chapter) -> Vec<Run> {
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
        match (folds(stop), out.last_mut()) {
            (false, _) => out.push(Run::Read(i)),
            (true, Some(Run::Skim(group))) => group.push(i),
            (true, _) => out.push(Run::Skim(vec![i])),
        }
    }
    out
}

/// Every stop of the tour in reading order, with its chapter.
pub fn stops(tour: &v4::Tour) -> impl Iterator<Item = (&v4::Chapter, &v4::Stop)> {
    tour.chapters
        .iter()
        .flat_map(|chapter| chapter.stops.iter().map(move |stop| (chapter, stop)))
}

/// The read stops of the tour, in reading order.
pub fn read_stops(tour: &v4::Tour) -> impl Iterator<Item = &v4::Stop> {
    stops(tour)
        .map(|(_, stop)| stop)
        .filter(|stop| !folds(stop))
}

/// The first read stop after the stop `id`, in reading order.
pub fn next_read(tour: &v4::Tour, id: &str) -> Option<String> {
    stops(tour)
        .map(|(_, stop)| stop)
        .skip_while(|stop| stop.id != id)
        .skip(1)
        .find(|stop| !folds(stop))
        .map(|stop| stop.id.clone())
}

/// A window of a hunk picture: 1-based inclusive line ranges on each side,
/// either absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub base: Option<(u32, u32)>,
    pub head: Option<(u32, u32)>,
}

impl From<&v4::Window> for Window {
    fn from(w: &v4::Window) -> Window {
        Window {
            base: w.base.as_ref().and_then(lines),
            head: w.head.as_ref().and_then(lines),
        }
    }
}

/// A hunk picture's file and windows.
pub fn hunk(picture: &PicturesItem) -> Option<(&str, Vec<Window>)> {
    match picture {
        PicturesItem::Hunk { file, windows, .. } => {
            Some((file, windows.iter().map(Window::from).collect()))
        }
        PicturesItem::Seq { .. } => None,
    }
}

/// A diagram Changescape laid out, as one SVG document. `hot` maps an SVG
/// node's `id` to the element it stands for.
#[derive(Debug, Clone, Default)]
pub struct Svg {
    pub svg: String,
    pub hot: BTreeMap<String, String>,
}

impl Svg {
    fn new(svg: &str, hot: &HashMap<String, String>) -> Svg {
        Svg {
            svg: svg.to_string(),
            hot: hot.clone().into_iter().collect(),
        }
    }
}

/// A seq picture's diagram.
pub fn picture_svg(picture: &PicturesItem) -> Option<Svg> {
    match picture {
        PicturesItem::Seq { svg, hot, .. } => Some(Svg::new(svg, hot)),
        PicturesItem::Hunk { .. } => None,
    }
}

/// A chapter's spine, as a diagram.
pub fn spine_svg(spine: &v4::SeqPicture) -> Svg {
    Svg::new(&spine.svg, &spine.hot)
}

/// A line range as 1-based `(first, last)`.
pub fn lines(range: &v4::Range) -> Option<(u32, u32)> {
    Some((
        u32::try_from(range.start).ok()?,
        u32::try_from(range.end).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Case 01 with `views`, and with `pr:1` and `pr:2` as elements.
    fn page(views: serde_json::Value) -> LoadedPage {
        let mut value: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/conformance/01-minimal/page.json"
        ))
        .unwrap();
        value["views"] = views;
        let add = value["elements"]["fn:calc.ops.add"].clone();
        value["elements"]["pr:1"] = add.clone();
        value["elements"]["pr:2"] = add;
        LoadedPage {
            page: serde_json::from_value(value).unwrap(),
        }
    }

    fn view(blocks: serde_json::Value) -> serde_json::Value {
        serde_json::json!([{ "id": "related", "section": "context", "title": "Related",
            "subtitle": "", "icon": "overview", "question": "", "metric": "", "narration": null,
            "blocks": blocks }])
    }

    fn row(el: Option<&str>) -> serde_json::Value {
        serde_json::json!({ "cells": {}, "dim": false, "element": el, "group": null,
            "rank": "normal", "side": null, "why": null })
    }

    fn table(id: &str, rows: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({ "type": "table", "id": id, "columns": [], "element": null,
            "fold": null, "groups": [], "note": null, "rows": rows, "title": null })
    }

    #[test]
    fn a_views_rows_are_the_elements_its_tables_name_each_once() {
        let page = page(view(serde_json::json!([
            table("a", vec![row(Some("pr:1")), row(None), row(Some("pr:2"))]),
            { "type": "text", "id": "t", "text": "x" },
            table("b", vec![row(Some("pr:1")), row(Some("pr:404"))]),
        ])));
        assert_eq!(page.view_rows(ViewId::Related), ["pr:1", "pr:2"]);
        assert!(page.view_rows(ViewId::Jobs).is_empty());
    }

    #[test]
    fn a_views_pictures_are_its_svg_blocks() {
        let page = page(view(serde_json::json!([
            { "type": "svg", "id": "a", "svg": "<svg/>", "hot": { "n": "job:x" },
              "note": null, "title": null },
            { "type": "narration", "id": "n", "text": "x" },
        ])));
        let pictures = page.view_pictures(ViewId::Related);
        assert_eq!(pictures.len(), 1);
        assert_eq!(
            pictures[0].hot,
            BTreeMap::from([("n".to_string(), "job:x".to_string())])
        );
    }

    #[test]
    fn a_text_block_reads_as_text_not_narration() {
        let block: BlocksItem =
            serde_json::from_value(serde_json::json!({ "type": "text", "id": "t", "text": "x" }))
                .unwrap();
        assert!(matches!(block, BlocksItem::Text { .. }));
    }

    fn chapter(verdicts: &[&str]) -> v4::Chapter {
        let stops: Vec<serde_json::Value> = verdicts
            .iter()
            .enumerate()
            .map(|(i, v)| {
                serde_json::json!({ "id": format!("s{i}"), "title": "t", "elements": [],
                    "pictures": [], "lines": 1, "verdict": v, "asides": [], "before": null,
                    "files": [], "hash": null, "now": null, "relies": [], "watch": null,
                    "watch_target": null })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": "c", "title": "C", "verdict": "read", "stops": stops, "hash": null,
            "intro": null, "lines": { "read": 0, "skim": 0 }, "parts": [], "sections": [],
            "spine": null }))
        .unwrap()
    }

    #[test]
    fn consecutive_skim_and_skip_stops_fold_into_one_run() {
        let c = chapter(&["read", "skim", "skip", "read", "skim"]);
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
        let mut c = chapter(&["read", "skim", "skim", "read", "read"]);
        c.sections = serde_json::from_value(serde_json::json!([
            { "title": "A", "stops": ["s0", "s1"], "intro": null },
            { "title": "B", "stops": ["s2", "s3"], "intro": null },
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
        let first = chapter(&["read", "skim"]);
        let mut second = chapter(&["skim", "read"]);
        for stop in &mut second.stops {
            stop.id = format!("t{}", stop.id);
        }
        let tour = v4::Tour {
            below: Vec::new(),
            chapters: vec![first, second],
            ending: None,
            flags: Vec::new(),
            map: Vec::new(),
            narrated: true,
            summary: None,
        };
        assert_eq!(next_read(&tour, "s0").as_deref(), Some("ts1"));
        assert_eq!(next_read(&tour, "s1").as_deref(), Some("ts1"));
        assert_eq!(next_read(&tour, "ts1"), None);
        let read: Vec<&str> = read_stops(&tour).map(|s| s.id.as_str()).collect();
        assert_eq!(read, ["s0", "ts1"]);
    }
}
