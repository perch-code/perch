//! The Changescape tour in the whole window, on the conformance fixtures
//! of `perch-changescape` (`tests/fixtures/conformance/`). Case 12 is a
//! tour of one chapter: a read stop, `multiply`, on head lines 1-3 of
//! `calc/ops.py`, and a skim stop, `multiply-helper`, on lines 4-5.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use floem::headless::HeadlessHarness;
use floem::reactive::{SignalGet, SignalUpdate};
use perch_github::{Side, StoredMark, TourMark};

use crate::backend::changescape::Found;
use crate::review::changescape::Shown;
use crate::review::header::tests::{named, pull_request, pull_request_harness_with, rect};
use crate::review::model::{ItemTab, ReviewState};

const REPO: &str = "calc/calc";
const HEAD: &str = "3e81042d07e4d404b84526777e768a456a6a4fa9";
const OPS: &str = "calc/ops.py";
const TOUR: &str = "12-tour";

/// A picture 200 by 100 whose box at (0, 0), 80 by 40, is `multiply()`.
const DIAGRAM: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><g id="k"><rect x="0" y="0" width="80" height="40" fill="#ccc"/></g></svg>"##;

fn cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../perch-changescape/tests/fixtures/conformance")
}

/// A cache root with case `name` as pull request 812 at `sha`.
fn cache_with(name: &str, sha: &str) -> tempfile::TempDir {
    cache_patched(name, sha, |_| {})
}

/// A cache root with case `name` as pull request 812 at `sha`, its page
/// changed by `patch`.
fn cache_patched(
    name: &str,
    sha: &str,
    patch: impl FnOnce(&mut serde_json::Value),
) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let to = perch_changescape::build_dir(root.path(), REPO, 812, sha);
    std::fs::create_dir_all(&to).unwrap();
    std::fs::copy(
        cases().join(name).join("status.json"),
        to.join("status.json"),
    )
    .unwrap();
    let mut page: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cases().join(name).join("page.json")).unwrap())
            .unwrap();
    patch(&mut page);
    std::fs::write(to.join("page.json"), page.to_string()).unwrap();
    root
}

/// Case 12 with what it lacks: a spine on the chapter, a flag on the
/// cover, text on the read stop, and a second read stop after the skim
/// one.
fn cache_with_more(sha: &str) -> tempfile::TempDir {
    cache_patched(TOUR, sha, |page| {
        let tour = &mut page["tour"];
        tour["flags"] = serde_json::json!([{ "kind": "warn", "title": "A flag",
            "text": "It changes how to read this.", "ref": "fn:calc.ops.multiply" }]);
        tour["ending"] = serde_json::json!({ "checks": [{ "q": "Does it match?", "a": "Yes." }],
            "actions": [{ "label": "Draft the review" }] });
        let chapter = &mut tour["chapters"][0];
        chapter["intro"] = "Where `multiply` comes from.".into();
        chapter["spine"] = serde_json::json!({ "kind": "seq", "svg": DIAGRAM,
            "hot": { "k": "fn:calc.ops.multiply" }, "title": "From the caller" });
        let stops = chapter["stops"].as_array_mut().unwrap();
        stops[0]["now"] = "It multiplies `a` by `b`.".into();
        stops[0]["watch"] = "The helper calls it.".into();
        stops[0]["watch_ref"] = "fn:calc.ops._multiply_helper".into();
        stops.push(serde_json::json!({
            "id": "again", "title": "Multiply again", "verdict": "read",
            "els": ["fn:calc.ops.multiply"], "lines": 1,
            "pictures": [{ "kind": "hunk", "file": OPS,
                "windows": [{ "base": null, "head": [2, 2] }] }],
        }));
    })
}

/// Show what the backend would find in `root`, changed by `edit`.
fn show(state: ReviewState, root: &Path, edit: impl FnOnce(&mut Found)) {
    let mut found = crate::backend::changescape::find(root, REPO, 812, HEAD);
    edit(&mut found);
    state
        .changescape
        .set(Some(Arc::new(Shown::new("pr".into(), Arc::new(found)))));
}

/// The window on pull request 812 at `HEAD`, with what the backend would
/// find in `root`.
fn harness_with_cache(root: Option<&Path>) -> (HeadlessHarness, ReviewState) {
    let mut pull = pull_request(812);
    pull.head_sha = HEAD.into();
    let (mut harness, _, state) = pull_request_harness_with(pull);
    if let Some(root) = root {
        show(state, root, |_| {});
    }
    redraw(&mut harness);
    (harness, state)
}

/// The tab open on the cache in `root`.
fn opened_in(root: tempfile::TempDir) -> (HeadlessHarness, ReviewState, tempfile::TempDir) {
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    (harness, state, root)
}

/// The tab open on case `name`.
fn opened(name: &str) -> (HeadlessHarness, ReviewState, tempfile::TempDir) {
    opened_in(cache_with(name, HEAD))
}

/// The tab open on case `name`, where this Perch can build or not.
fn opened_building(
    name: &str,
    can_build: bool,
) -> (HeadlessHarness, ReviewState, tempfile::TempDir) {
    let root = cache_with(name, HEAD);
    let (mut harness, state) = harness_with_cache(None);
    show(state, root.path(), |found| found.can_build = can_build);
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    (harness, state, root)
}

/// The tab on case 12 with `marks`, each stop's mark done with its hash.
fn with_marks(root: &Path, marks: &[(&str, Option<&str>)]) -> (HeadlessHarness, ReviewState) {
    let (mut harness, state) = harness_with_cache(None);
    show(state, root, |found| {
        for (stop, h) in marks {
            found.marks.insert(
                stop.to_string(),
                StoredMark {
                    mark: TourMark::Done,
                    h: h.map(str::to_string),
                },
            );
        }
    });
    redraw(&mut harness);
    (harness, state)
}

/// `calc/ops.py` as Code loads it under the Branch scope: case 12's five
/// lines, all added.
fn load_ops(state: ReviewState) {
    let file = crate::backend::build_loaded_file(
        OPS.into(),
        "def multiply(a, b):\n    return a * b\n\ndef _multiply_helper(a, b):\n    return multiply(a, b)\n"
            .into(),
        String::new(),
        Some(perch_git::ChangeStatus::Added),
        false,
        perch_git::Scope::Branch,
        String::new(),
    );
    state.scope.set(perch_git::Scope::Branch);
    state.files.update(|files| {
        files.insert(OPS.to_string(), Arc::new(file));
    });
}

fn redraw(harness: &mut HeadlessHarness) {
    harness.rebuild();
    harness.paint();
}

/// The names of a view's children, without the kind of view floem puts
/// after each.
fn child_names(id: floem::ViewId) -> Vec<String> {
    id.children()
        .into_iter()
        .map(|child| {
            let name = child.debug_name();
            name.split(" - ").next().unwrap_or_default().to_string()
        })
        .collect()
}

fn switch_children(harness: &HeadlessHarness) -> Vec<String> {
    child_names(named(harness.root_id(), "item switch").expect("no switch"))
}

fn click(harness: &mut HeadlessHarness, name: &str) {
    let at = rect(harness, name).center();
    harness.click(at.x, at.y);
    redraw(harness);
}

/// Every view whose debug name has `name` in it, depth first.
fn all_named(id: floem::ViewId, name: &str, found: &mut Vec<floem::ViewId>) {
    if id.debug_name().contains(name) {
        found.push(id);
    }
    for child in id.children() {
        all_named(child, name, found);
    }
}

/// How many views named `name` are under `id` and drawn with a size.
fn shown_under(id: floem::ViewId, name: &str) -> usize {
    let mut found = Vec::new();
    all_named(id, name, &mut found);
    found
        .iter()
        .filter(|v| v.get_visual_rect_no_clip().height() > 0.0)
        .count()
}

fn stop(harness: &HeadlessHarness, id: &str) -> floem::ViewId {
    named(harness.root_id(), &format!("Changescape stop {id}"))
        .unwrap_or_else(|| panic!("no stop {id}"))
}

/// The first view named `name` inside the stop `id`.
fn in_stop_view(harness: &HeadlessHarness, id: &str, name: &str) -> floem::ViewId {
    let mut found = Vec::new();
    all_named(stop(harness, id), name, &mut found);
    *found
        .first()
        .unwrap_or_else(|| panic!("no {name} in stop {id}"))
}

fn in_stop(harness: &HeadlessHarness, id: &str, name: &str) -> floem::kurbo::Rect {
    in_stop_view(harness, id, name).get_visual_rect_no_clip()
}

/// Click a view, once the tour has scrolled it into sight.
fn click_view(harness: &mut HeadlessHarness, view: floem::ViewId) {
    view.scroll_to(None);
    redraw(harness);
    redraw(harness);
    click_at(harness, view.get_visual_rect_no_clip());
}

/// Click the first view named `name` in the stop `id`.
fn click_in_stop(harness: &mut HeadlessHarness, id: &str, name: &str) {
    let view = in_stop_view(harness, id, name);
    click_view(harness, view);
}

fn click_at(harness: &mut HeadlessHarness, at: floem::kurbo::Rect) {
    let at = at.center();
    harness.click(at.x, at.y);
    redraw(harness);
}

#[test]
fn with_no_changescape_the_switch_is_as_it_was() {
    let empty = tempfile::tempdir().unwrap();
    for root in [None, Some(empty.path())] {
        let (harness, _) = harness_with_cache(root);
        assert!(named(harness.root_id(), "Changescape pill").is_none());
        assert_eq!(
            switch_children(&harness),
            ["Conversation pill", "Code pill"]
        );
    }
}

#[test]
fn case_12_adds_the_pill_and_opens_on_the_tour() {
    let root = cache_with(TOUR, HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    assert_eq!(
        switch_children(&harness),
        ["Conversation pill", "Changescape pill", "Code pill"]
    );
    click(&mut harness, "Changescape pill");
    assert_eq!(state.tab.get(), ItemTab::Changescape);
    for part in [
        "Changescape cover",
        "Changescape lines bar",
        "Changescape contents",
        "Changescape chapter change",
        "Changescape ending",
    ] {
        assert!(rect(&harness, part).height() > 0.0, "{part}");
    }
    // The read stop is open, with its buttons; the skim stop is a row.
    assert!(in_stop(&harness, "multiply", "Changescape done").height() > 0.0);
    assert!(named(harness.root_id(), "Changescape skim group").is_some());
    assert_eq!(
        shown_under(stop(&harness, "multiply-helper"), "Changescape done"),
        0
    );
    // Narrated: no note about the narration to come.
    assert!(named(harness.root_id(), "Changescape narration pending").is_none());
    // The page's line numbers are the branch's.
    assert_eq!(state.scope.get(), perch_git::Scope::Branch);
}

#[test]
fn a_changescape_of_another_item_shows_nothing() {
    let root = cache_with(TOUR, HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    state.changescape.update(|shown| {
        let found = shown.as_ref().unwrap().found.clone();
        *shown = Some(Arc::new(Shown::new("other".into(), found)));
    });
    redraw(&mut harness);
    assert!(named(harness.root_id(), "Changescape pill").is_none());
}

#[test]
fn a_tour_not_yet_narrated_says_the_narration_comes_later() {
    let root = cache_patched(TOUR, HEAD, |page| {
        page["tour"]["narrated"] = false.into();
    });
    let (harness, _, _root) = opened_in(root);
    assert!(rect(&harness, "Changescape narration pending").height() > 0.0);
}

#[test]
fn a_stops_window_is_cut_from_the_file_and_a_line_opens_code_there() {
    let root = cache_with(TOUR, HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    load_ops(state);
    click(&mut harness, "Changescape pill");
    redraw(&mut harness);
    assert_eq!(
        shown_under(stop(&harness, "multiply"), "Changescape code line"),
        3,
        "head lines 1 to 3"
    );
    let mut lines = Vec::new();
    all_named(
        stop(&harness, "multiply"),
        "Changescape code line",
        &mut lines,
    );
    click_view(&mut harness, lines[1]);
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(state.active.get().as_deref(), Some(OPS));
    // The file is loaded, so the editor takes the jump at once.
    redraw(&mut harness);
    assert_eq!(state.cursor_line.get(), 1, "line 2, from 0");
}

#[test]
fn a_skim_row_opens_its_stop_in_place() {
    let root = cache_with(TOUR, HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    load_ops(state);
    click(&mut harness, "Changescape pill");
    let helper = stop(&harness, "multiply-helper");
    assert_eq!(shown_under(helper, "Changescape code line"), 0);
    click(&mut harness, "Changescape skim row multiply-helper");
    redraw(&mut harness);
    assert!(state.changescape_opened.get().contains("multiply-helper"));
    assert_eq!(
        shown_under(stop(&harness, "multiply-helper"), "Changescape code line"),
        2
    );
    click(&mut harness, "Changescape skim row multiply-helper");
    assert_eq!(
        shown_under(stop(&harness, "multiply-helper"), "Changescape code line"),
        0
    );
}

#[test]
fn done_and_come_back_mark_a_stop_and_a_second_click_clears_it() {
    let (mut harness, state, _root) = opened(TOUR);
    click_in_stop(&mut harness, "multiply", "Changescape done");
    assert_eq!(
        state.changescape_marks.get().get("multiply"),
        Some(&TourMark::Done)
    );
    let bar = crate::review::changescape::tour::progress_line(
        state,
        state.changescape.get().unwrap().page().unwrap(),
    );
    assert_eq!(bar, "1 of 1 read stops done");
    click_in_stop(&mut harness, "multiply", "Changescape later");
    assert_eq!(
        state.changescape_marks.get().get("multiply"),
        Some(&TourMark::Later),
        "one mark at a time"
    );
    click_in_stop(&mut harness, "multiply", "Changescape later");
    assert!(state.changescape_marks.get().is_empty());
}

#[test]
fn marks_the_backend_found_are_shown() {
    let root = cache_with(TOUR, HEAD);
    let (_harness, state) = with_marks(root.path(), &[("multiply", None)]);
    assert_eq!(
        state.changescape_marks.get().get("multiply"),
        Some(&TourMark::Done)
    );
}

#[test]
fn a_stop_whose_hash_changed_since_its_mark_shows_as_changed_until_marked_again() {
    let root = cache_patched(TOUR, HEAD, |page| {
        page["tour"]["chapters"][0]["stops"][0]["h"] = "new".into();
        page["tour"]["chapters"][0]["stops"][1]["h"] = "same".into();
    });
    let (mut harness, state) = with_marks(
        root.path(),
        &[("multiply", Some("old")), ("multiply-helper", Some("same"))],
    );
    click(&mut harness, "Changescape pill");
    assert_eq!(
        state.changescape_changed.get().get("multiply"),
        Some(&TourMark::Done)
    );
    assert!(!state.changescape_marks.get().contains_key("multiply"));
    assert_eq!(
        state.changescape_marks.get().get("multiply-helper"),
        Some(&TourMark::Done),
        "the same hash keeps its mark"
    );
    assert!(in_stop(&harness, "multiply", "Changescape changed since marked").height() > 0.0);
    assert!(rect(&harness, "Changescape left").height() > 0.0);

    click_in_stop(&mut harness, "multiply", "Changescape done");
    assert!(state.changescape_changed.get().is_empty());
    assert_eq!(
        state.changescape_marks.get().get("multiply"),
        Some(&TourMark::Done)
    );
    assert_eq!(
        in_stop(&harness, "multiply", "Changescape changed since marked").height(),
        0.0
    );
}

#[test]
fn a_chapter_with_sections_draws_a_header_before_each_and_labels_the_outline() {
    let root = cache_patched(TOUR, HEAD, |page| {
        page["tour"]["chapters"][0]["sections"] = serde_json::json!([
            { "title": "The product", "intro": "What it multiplies.", "stops": ["multiply"] },
            { "title": "The helper", "stops": ["multiply-helper"] },
        ]);
    });
    let (harness, _, _root) = opened_in(root);
    for name in [
        "Changescape section The product",
        "Changescape section The helper",
    ] {
        assert!(rect(&harness, name).height() > 0.0, "{name}");
    }
    let mut labels = Vec::new();
    all_named(
        harness.root_id(),
        "Changescape outline section",
        &mut labels,
    );
    assert_eq!(labels.len(), 2);
}

#[test]
fn a_stop_shows_what_it_relies_on_and_its_asides_and_code_opens_the_lines() {
    let root = cache_patched(TOUR, HEAD, |page| {
        let stop = &mut page["tour"]["chapters"][0]["stops"][0];
        stop["relies"] = serde_json::json!([{
            "el": "fn:calc.ops._multiply_helper", "why": "calls", "sig": "def _multiply_helper(a, b)",
            "doc": "Multiply through the helper. More text.", "code": { "file": OPS, "head": [4, 5] } }]);
        stop["asides"] = serde_json::json!([{
            "view": "related", "q": "Does an open PR touch this?", "a": "One does.",
            "rows": ["fn:calc.ops.multiply", "pr:404"] }]);
        page["tour"]["ending"] = serde_json::json!({ "checks": [{
            "q": "Plans that name no code?", "a": "One.", "view": "plans",
            "rows": ["fn:calc.ops.multiply"] }] });
    });
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    load_ops(state);
    click(&mut harness, "Changescape pill");
    assert!(
        in_stop(
            &harness,
            "multiply",
            "Changescape rely fn:calc.ops._multiply_helper"
        )
        .height()
            > 0.0
    );
    assert!(in_stop(&harness, "multiply", "Changescape aside").height() > 0.0);
    // The aside names one element the page holds; the PR it does not is left out.
    assert_eq!(
        shown_under(
            in_stop_view(&harness, "multiply", "Changescape aside"),
            "Changescape row element"
        ),
        1
    );
    let check = named(harness.root_id(), "Changescape check").unwrap();
    assert_eq!(shown_under(check, "Changescape row element"), 1);

    let before = shown_under(stop(&harness, "multiply"), "Changescape code line");
    click_in_stop(&mut harness, "multiply", "Changescape rely code toggle");
    redraw(&mut harness);
    assert_eq!(
        shown_under(stop(&harness, "multiply"), "Changescape code line"),
        before + 2,
        "head lines 4 and 5"
    );
}

#[test]
fn next_goes_to_the_next_read_stop_past_the_skim_ones_then_the_ending() {
    let (mut harness, state, _root) = opened_in(cache_with_more(HEAD));
    click_in_stop(&mut harness, "multiply", "Changescape next");
    assert!(state.changescape_opened.get().contains("again"));
    assert!(
        !state.changescape_opened.get().contains("multiply-helper"),
        "a skim stop is passed over"
    );
    assert!(
        state.changescape_marks.get().is_empty(),
        "Next marks nothing"
    );
    // The last read stop's Next goes to the ending, which opens nothing.
    click_in_stop(&mut harness, "again", "Changescape next");
    assert!(!state.changescape_opened.get().contains(super::side::ENDING));
}

#[test]
fn the_cover_has_the_flag_and_the_ending_its_checks_and_actions() {
    let (harness, _, _root) = opened_in(cache_with_more(HEAD));
    assert!(rect(&harness, "Changescape flag").height() > 0.0);
    assert!(rect(&harness, "Changescape check").height() > 0.0);
    assert!(rect(&harness, "Changescape action").height() > 0.0);
    assert!(in_stop(&harness, "multiply", "Changescape field now").height() > 0.0);
    assert!(in_stop(&harness, "multiply", "Changescape field watch").height() > 0.0);
}

#[test]
fn a_watch_ref_to_an_element_opens_it_in_code() {
    let (mut harness, state, _root) = opened_in(cache_with_more(HEAD));
    click_in_stop(&mut harness, "multiply", "Changescape ref");
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(state.active.get().as_deref(), Some(OPS));
    assert_eq!(state.jump_caret.get(), Some((3, 0)), "the helper's line 4");
}

/// The window on the tour with a spine, laid out once more so that the
/// diagram fits the column.
fn with_spine() -> (HeadlessHarness, ReviewState, tempfile::TempDir) {
    let (mut harness, state, root) = opened_in(cache_with_more(HEAD));
    redraw(&mut harness);
    // The spine comes after the cover: in sight first.
    let spine = named(harness.root_id(), "Changescape picture").expect("no spine");
    spine.scroll_to(None);
    redraw(&mut harness);
    redraw(&mut harness);
    (harness, state, root)
}

fn nth(harness: &HeadlessHarness, name: &str, n: usize) -> floem::kurbo::Rect {
    let mut found = Vec::new();
    all_named(harness.root_id(), name, &mut found);
    found
        .get(n)
        .unwrap_or_else(|| panic!("no view {n} named {name}"))
        .get_visual_rect_no_clip()
}

#[test]
fn a_hot_node_in_the_spine_opens_its_element() {
    let (mut harness, state, _root) = with_spine();
    let diagram = nth(&harness, "Changescape diagram", 0);
    let z = diagram.width() / 200.0;
    assert!(z > 0.5, "fitted to the column, not left tiny: {z}");
    let (x, y) = (diagram.x0 + 40.0 * z, diagram.y0 + 20.0 * z);
    harness.pointer_move(x, y);
    harness.click(x, y);
    redraw(&mut harness);
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(state.active.get().as_deref(), Some(OPS));
    assert_eq!(state.jump_caret.get(), Some((0, 0)));
}

/// The diagram in the overlay.
fn expanded_diagram(harness: &HeadlessHarness) -> floem::kurbo::Rect {
    let overlay = named(harness.root_id(), "Changescape expanded diagram").expect("no overlay");
    let mut found = Vec::new();
    all_named(overlay, "Changescape diagram", &mut found);
    found
        .first()
        .expect("no diagram in the overlay")
        .get_visual_rect_no_clip()
}

#[test]
fn expand_draws_the_spine_larger_under_the_commit_bar() {
    let (mut harness, state, _root) = with_spine();
    let small = nth(&harness, "Changescape diagram", 0);
    click(&mut harness, "Changescape expand");
    redraw(&mut harness);
    assert_eq!(
        state.changescape_expanded.get().as_deref(),
        Some("spine:change")
    );
    let overlay = rect(&harness, "Changescape expanded diagram");
    let bar = rect(&harness, "Changescape commit bar");
    assert!(overlay.y0 >= bar.y1 - 0.5, "the commit bar stays in sight");
    let large = expanded_diagram(&harness);
    assert!(
        large.width() > small.width(),
        "{} is not larger than {}",
        large.width(),
        small.width()
    );
    assert!(large.height() <= overlay.height());
}

#[test]
fn escape_and_collapse_put_an_expanded_diagram_away() {
    use floem::prelude::{Code, Key, KeyState, KeyboardEvent, Modifiers, NamedKey};
    let (mut harness, state, _root) = with_spine();
    click(&mut harness, "Changescape expand");
    harness.dispatch_event(floem::event::Event::Key(KeyboardEvent {
        state: KeyState::Down,
        key: Key::Named(NamedKey::Escape),
        code: Code::Escape,
        location: Default::default(),
        modifiers: Modifiers::empty(),
        repeat: false,
        is_composing: false,
    }));
    redraw(&mut harness);
    assert_eq!(state.changescape_expanded.get(), None);
    assert_eq!(state.tab.get(), ItemTab::Changescape);
    click(&mut harness, "Changescape expand");
    assert!(state.changescape_expanded.get().is_some());
    click(&mut harness, "Changescape collapse");
    assert_eq!(state.changescape_expanded.get(), None);
}

#[test]
fn a_thread_on_a_stops_lines_shows_on_that_stop() {
    let root = cache_with(TOUR, HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    let thread = |id, line| super::stop::tests::thread(id, OPS, (line, line), Side::Right);
    state
        .comment_threads
        .set(Arc::new(vec![thread("gh-1", 2), thread("gh-2", 9)]));
    click(&mut harness, "Changescape pill");
    let threads = named(stop(&harness, "multiply"), "Changescape stop threads").unwrap();
    let cards = threads
        .children()
        .first()
        .map_or(0, |list| list.children().len());
    assert_eq!(cards, 1, "the thread on line 2, not the one on line 9");
}

#[test]
fn chat_about_this_opens_the_composer_on_the_stop_with_its_context() {
    let (mut harness, state, _root) = opened_in(cache_with_more(HEAD));
    click_in_stop(&mut harness, "multiply", "Changescape chat");
    let composer = state.composer.get().expect("no composer");
    assert_eq!(composer.about.as_deref(), Some("tour:multiply"));
    assert!(composer.thread_id.is_none(), "a new thread");
    let context = composer.context.unwrap_or_default();
    assert!(context.contains("Stop: Multiply two numbers"), "{context}");
    assert!(context.contains("Now: It multiplies a by b."), "{context}");
    let anchor = composer.anchor.expect("no anchor");
    assert_eq!(
        (anchor.path.as_str(), anchor.start_line, anchor.line),
        (OPS, 1, 3)
    );
    redraw(&mut harness);
    assert!(in_stop(&harness, "multiply", "Changescape stop composer").height() > 0.0);
    assert!(
        named(harness.root_id(), "composer").is_none() || state.tab.get() == ItemTab::Changescape,
        "it is not also Conversation's"
    );
}

#[test]
fn the_caret_in_a_stops_window_shows_the_code_strip_which_leads_back() {
    let root = cache_with(TOUR, HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    load_ops(state);
    state.active.set(Some(OPS.to_string()));
    state.show_tab(ItemTab::Code);
    // Drawn once first: opening the file puts the caret at its top.
    redraw(&mut harness);
    // Line 4, in the skim stop.
    state.cursor_line.set(3);
    redraw(&mut harness);
    assert!(rect(&harness, "Changescape code strip").height() > 0.0);
    click(&mut harness, "Changescape strip link");
    assert_eq!(state.tab.get(), ItemTab::Changescape);
    assert!(
        state.changescape_opened.get().contains("multiply-helper"),
        "the skim stop opens in place"
    );
}

#[test]
fn a_page_without_a_tour_says_why_and_offers_a_rebuild_where_one_can_run() {
    for (case, can_build) in [
        ("01-minimal", true),
        ("13-malformed-tour", true),
        ("01-minimal", false),
    ] {
        let (harness, state, _root) = opened_building(case, can_build);
        assert_eq!(state.tab.get(), ItemTab::Changescape, "{case}");
        assert!(
            named(harness.root_id(), "Changescape refused").is_some(),
            "{case}"
        );
        assert!(
            named(harness.root_id(), "Changescape tour").is_none(),
            "{case}"
        );
        let offered = named(harness.root_id(), "Changescape rebuild")
            .is_some_and(|v| v.get_visual_rect_no_clip().width() > 0.0);
        assert_eq!(offered, can_build, "{case}");
    }
}

/// Case 12 with a view of related work, Related, that a flag points at:
/// a table whose rows name a pull request, a commit, a function, and an
/// element the page does not hold.
fn cache_with_view(sha: &str) -> tempfile::TempDir {
    cache_patched(TOUR, sha, |page| {
        page["elements"]["pr:5"] = serde_json::json!({
            "t": "#5 Divide too", "k": "pull request", "st": "context",
            "d": "by someone \u{b7} open", "refs": [],
            "url": "https://github.com/acme/calc/pull/5" });
        page["elements"]["commit:abc1234"] = serde_json::json!({
            "t": "abc1234 \u{b7} Add multiply", "k": "commit", "st": "changed", "refs": [],
            "url": "https://github.com/acme/calc/commit/abc1234" });
        page["views"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
            "id": "related", "sec": "surroundings", "t": "Related work", "s": "",
            "ic": "pr", "q": "", "state": "applies", "metric": "1 PR", "narration": null,
            "blocks": [
                { "type": "table", "columns": [{ "key": "t", "label": "T" }], "rows": [
                    { "el": "pr:5", "cells": { "t": "#5" } },
                    { "el": "commit:abc1234", "cells": { "t": "abc" } },
                    { "el": "fn:calc.ops.multiply", "cells": { "t": "multiply" } },
                    { "el": "pr:404", "cells": { "t": "gone" } } ] },
                { "type": "sparkline", "rows": [{ "el": "pr:5" }] },
            ] }));
        page["tour"]["flags"] = serde_json::json!([{ "kind": "info", "title": "Others work here",
            "text": "An open PR touches the same file.", "ref": "view:related" }]);
    })
}

fn view_rows(harness: &HeadlessHarness) -> Vec<String> {
    let card = named(harness.root_id(), "Changescape view related").expect("no popover");
    let mut rows = Vec::new();
    all_named(card, "Changescape view row", &mut rows);
    rows.iter()
        .map(|r| {
            r.debug_name()
                .split(" - ")
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[test]
fn a_view_ref_shows_the_views_rows_as_its_elements() {
    let (mut harness, state, _root) = opened_in(cache_with_view(HEAD));
    assert!(named(harness.root_id(), "Changescape view related").is_none());
    let link = named(harness.root_id(), "Changescape ref").expect("no ref");
    click_view(&mut harness, link);
    assert_eq!(state.changescape_view.get().as_deref(), Some("related"));
    assert_eq!(
        view_rows(&harness),
        [
            "Changescape view row pr:5",
            "Changescape view row commit:abc1234",
            "Changescape view row fn:calc.ops.multiply",
        ],
        "each element once, in row order, and none the page lacks"
    );
    // Close puts it away, and so does a click beside it.
    click(&mut harness, "Changescape view close");
    assert_eq!(state.changescape_view.get(), None);
    state.changescape_view.set(Some("related".into()));
    redraw(&mut harness);
    let card = rect(&harness, "Changescape view related");
    harness.click(card.x0 - 8.0, card.y0 + 8.0);
    redraw(&mut harness);
    assert_eq!(state.changescape_view.get(), None);
}

#[test]
fn a_code_row_in_a_view_opens_code_and_closes_the_popover() {
    let (mut harness, state, _root) = opened_in(cache_with_view(HEAD));
    state.changescape_view.set(Some("related".into()));
    redraw(&mut harness);
    click(&mut harness, "Changescape view row fn:calc.ops.multiply");
    assert_eq!(state.changescape_view.get(), None);
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(state.active.get().as_deref(), Some(OPS));
    assert_eq!(state.jump_caret.get(), Some((0, 0)));
}

#[test]
fn a_pull_request_row_opens_that_pull_request_in_perch() {
    let mut pull = pull_request(812);
    pull.head_sha = HEAD.into();
    let (mut harness, app, state) = pull_request_harness_with(pull);
    let root = cache_with_view(HEAD);
    show(state, root.path(), |_| {});
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    state.changescape_view.set(Some("related".into()));
    redraw(&mut harness);
    crate::ui::widgets::opened();
    click(&mut harness, "Changescape view row pr:5");
    assert_eq!(state.changescape_view.get(), None);
    assert!(
        crate::ui::widgets::opened().is_empty(),
        "not the browser: Perch opens it"
    );
    assert_eq!(app.open_item.get(), None, "the open item gives way");
}

#[test]
fn a_commit_the_checkout_lacks_opens_on_github() {
    let (mut harness, state, _root) = opened_in(cache_with_view(HEAD));
    state.changescape_view.set(Some("related".into()));
    redraw(&mut harness);
    crate::ui::widgets::opened();
    click(&mut harness, "Changescape view row commit:abc1234");
    assert_eq!(
        crate::ui::widgets::opened(),
        ["https://github.com/acme/calc/commit/abc1234"]
    );
}

/// A status as the schema has it: of `state`, with `error`.
fn status_json(state: &str, error: Option<&str>) -> String {
    serde_json::json!({
        "schema": 2, "state": state, "started": "2026-10-08T12:00:00Z", "finished": null,
        "stage": "facts", "error": error, "skill_version": "3.0.0",
    })
    .to_string()
}

/// What the backend sends for a build in `json`, over case `earlier` at
/// an earlier head when there is one, and `can_build`.
fn shown_build(
    state: ReviewState,
    json: &str,
    earlier: Option<&str>,
    can_build: bool,
) -> tempfile::TempDir {
    let root = match earlier {
        Some(case) => cache_with(case, "aaaaaaa1111"),
        None => tempfile::tempdir().unwrap(),
    };
    let dir = perch_changescape::build_dir(root.path(), REPO, 812, HEAD);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("status.json"), json).unwrap();
    show(state, root.path(), |found| found.can_build = can_build);
    root
}

#[test]
fn a_first_build_running_shows_the_pill_and_the_building_view() {
    let (mut harness, state) = harness_with_cache(None);
    let _root = shown_build(state, &status_json("running", None), None, true);
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape building").is_some());
    assert!(named(harness.root_id(), "Changescape tour").is_none());
}

#[test]
fn a_tour_of_an_earlier_head_is_shown_as_stale() {
    let root = cache_with(TOUR, "aaaaaaa1111");
    let (harness, state, _root) = opened_in(root);
    assert!(named(harness.root_id(), "Changescape tour").is_some());
    assert!(rect(&harness, "Changescape commit bar").height() > 0.0);
    assert!(state.changescape.get().unwrap().stale());
}

#[test]
fn a_build_over_an_earlier_page_keeps_the_page_and_says_so() {
    let (mut harness, state) = harness_with_cache(None);
    let _root = shown_build(state, &status_json("running", None), Some(TOUR), true);
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape tour").is_some());
    assert!(rect(&harness, "Changescape commit bar").height() > 0.0);
    assert!(state.changescape.get().unwrap().stale());
    assert_eq!(
        rect(&harness, "Changescape rebuild").width(),
        0.0,
        "no second build while one runs"
    );
}

#[test]
fn a_build_over_an_earlier_page_with_no_tour_shows_its_progress() {
    let (mut harness, state) = harness_with_cache(None);
    let _root = shown_build(
        state,
        &status_json("running", None),
        Some("01-minimal"),
        true,
    );
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape building").is_some());
    assert!(named(harness.root_id(), "Changescape refused").is_none());
}

#[test]
fn a_failed_build_offers_retry_only_where_a_build_can_run() {
    let failed = status_json("failed", Some("uv: command not found"));
    let (mut harness, state) = harness_with_cache(None);
    let _root = shown_build(state, &failed, None, true);
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape failed").is_some());
    assert!(rect(&harness, "Changescape retry").width() > 0.0);

    let (mut harness, state) = harness_with_cache(None);
    let _root = shown_build(state, &failed, None, false);
    redraw(&mut harness);
    assert!(
        named(harness.root_id(), "Changescape pill").is_none(),
        "nothing to show, and nothing to do about it"
    );
}

#[test]
fn the_row_menu_offers_a_build_only_where_one_can_run() {
    let (_, state) = harness_with_cache(None);
    let item = (*state.item.get().unwrap()).clone();
    let app = crate::state::AppState::new();
    let backend = std::rc::Rc::new(crate::backend::Backend::detached());
    crate::review::changescape::set_can_build(false);
    let menu = crate::ui::items::item_menu(app, backend.clone(), item.clone());
    assert!(!menu
        .outline()
        .iter()
        .any(|line| line.contains("Build Changescape")));
    crate::review::changescape::set_can_build(true);
    let menu = crate::ui::items::item_menu(app, backend, item);
    assert!(menu
        .outline()
        .iter()
        .any(|line| line.contains("Build Changescape")));
}

#[test]
fn a_page_that_does_not_read_shows_why() {
    let root = cache_patched(TOUR, HEAD, |page| {
        // A status must be a string, though its set is open.
        page["elements"]["fn:calc.ops.multiply"]["st"] = 5.into();
    });
    let (mut harness, _state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape failed").is_some());
}

#[test]
fn the_outline_lists_the_tour_and_a_click_goes_to_a_stop() {
    let (mut harness, state, _root) = opened(TOUR);
    for name in [
        "Changescape outline cover",
        "Changescape outline change",
        "Changescape outline stop multiply",
        "Changescape outline stop multiply-helper",
        "Changescape outline ending",
    ] {
        assert!(rect(&harness, name).height() > 0.0, "{name}");
    }
    click(&mut harness, "Changescape outline stop multiply-helper");
    assert!(
        state.changescape_opened.get().contains("multiply-helper"),
        "a skim stop opens in place"
    );
}

#[test]
fn the_code_map_lists_the_files_and_a_file_goes_to_its_stop() {
    let (mut harness, state, _root) = opened(TOUR);
    assert!(rect(&harness, "Changescape code map").width() > 0.0);
    click(&mut harness, "Changescape map file calc/ops.py");
    assert!(state.changescape_opened.get().contains("multiply"));
}

#[test]
fn a_file_in_no_stop_opens_in_code() {
    let root = cache_patched(TOUR, HEAD, |page| {
        page["tour"]["map"][0]["files"]
            .as_array_mut()
            .unwrap()
            .push("calc/other.py".into());
        let mut other = page["files"][0].clone();
        other["path"] = "calc/other.py".into();
        other["old"] = "calc/other.py".into();
        page["files"].as_array_mut().unwrap().push(other);
    });
    let (mut harness, state, _root) = opened_in(root);
    click(&mut harness, "Changescape map file calc/other.py");
    assert_eq!(state.tab.get(), ItemTab::Code);
}

#[test]
fn scrolling_to_a_stop_lights_it_up_on_both_sides() {
    let (mut harness, state, _root) = opened_in(cache_with_more(HEAD));
    assert_eq!(state.changescape_here.get(), None);
    let scroll = |harness: &mut HeadlessHarness, name: &str| {
        named(harness.root_id(), name).unwrap().scroll_to(None);
        redraw(harness);
        redraw(harness);
    };
    // At the end of the tour, the last stop has passed a third of the
    // column, and the ending has not.
    scroll(&mut harness, "Changescape ending");
    assert_eq!(state.changescape_here.get().as_deref(), Some("again"));
    scroll(&mut harness, "Changescape cover");
    assert_eq!(state.changescape_here.get(), None);
}

#[test]
fn a_skill_from_before_the_tour_offers_no_build_and_says_why() {
    let (mut harness, state) = harness_with_cache(None);
    let root = tempfile::tempdir().unwrap();
    show(state, root.path(), |found| found.skill_too_old = true);
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape unbuilt").is_some());
    assert_eq!(rect(&harness, "Changescape build button").width(), 0.0);
    assert!(rect(&harness, "Changescape skill too old").height() > 0.0);
}
