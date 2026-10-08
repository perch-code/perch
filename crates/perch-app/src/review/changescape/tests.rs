//! The Changescape tab in the whole window, on the conformance fixtures
//! of `perch-changescape` (`tests/fixtures/conformance/`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use floem::headless::HeadlessHarness;
use floem::reactive::{SignalGet, SignalUpdate};

use crate::review::changescape::Shown;
use crate::review::header::tests::{named, pull_request, pull_request_harness_with, rect};
use crate::review::model::{ItemTab, ReviewState};

const REPO: &str = "calc/calc";
const HEAD: &str = "3e81042d07e4d404b84526777e768a456a6a4fa9";
const OPS: &str = "calc/ops.py";

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

/// The minimal case with two things it lacks: `old()`, a function only on
/// the base side, as a second Placement row, and a Diagram view in Map
/// whose box at (0, 0), 80 by 40, is `add()`.
fn cache_with_more(sha: &str) -> tempfile::TempDir {
    cache_patched("01-minimal", sha, |page| {
        page["elements"]["fn:calc.ops.old"] = serde_json::json!({
            "t": "old()", "k": "function · calc.ops", "st": "removed",
            "refs": [{ "f": OPS, "base": [5, 6], "head": null,
                       "src": [[5, null, "-", "def old():"], [6, null, "-", "    pass"]] }],
        });
        let placement = page["views"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|view| view["id"] == "placement")
            .unwrap();
        placement["blocks"][0]["rows"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "el": "fn:calc.ops.old",
                "cells": { "fn": { "text": "old()", "el": "fn:calc.ops.old" }, "st": "removed" },
            }));
        page["views"].as_array_mut().unwrap().push(serde_json::json!({
            "id": "diagram", "sec": "map", "t": "Diagram", "s": "", "ic": "map", "q": "",
            "state": "applies", "metric": "", "narration": null,
            "blocks": [{ "type": "svg", "hot": { "k": "fn:calc.ops.add" },
                "svg": r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><g id="k"><rect x="0" y="0" width="80" height="40" fill="#ccc"/></g></svg>"## }],
        }));
        let map = page["sections"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|section| section["id"] == "map")
            .unwrap();
        map["views"].as_array_mut().unwrap().push("diagram".into());
    })
}

/// The window on pull request 812 at `HEAD`, with what the backend would
/// find in `root`.
fn harness_with_cache(root: Option<&Path>) -> (HeadlessHarness, ReviewState) {
    let mut pull = pull_request(812);
    pull.head_sha = HEAD.into();
    let (mut harness, _, state) = pull_request_harness_with(pull);
    if let Some(root) = root {
        let found = crate::backend::changescape::find(root, REPO, 812, HEAD);
        state
            .changescape
            .set(Some(Arc::new(Shown::new("pr".into(), Arc::new(found)))));
    }
    redraw(&mut harness);
    (harness, state)
}

/// The tab open on case `name`.
fn opened(name: &str) -> (HeadlessHarness, ReviewState, tempfile::TempDir) {
    let root = cache_with(name, HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    (harness, state, root)
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

fn nth(harness: &HeadlessHarness, name: &str, n: usize) -> floem::kurbo::Rect {
    let mut found = Vec::new();
    all_named(harness.root_id(), name, &mut found);
    found
        .get(n)
        .unwrap_or_else(|| panic!("no view {n} named {name}"))
        .get_visual_rect_no_clip()
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
fn case_01_adds_the_pill_and_opens_on_overview() {
    let root = cache_with("01-minimal", HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    assert_eq!(
        switch_children(&harness),
        ["Conversation pill", "Changescape pill", "Code pill"]
    );
    click(&mut harness, "Changescape pill");
    assert_eq!(state.tab.get(), ItemTab::Changescape);
    assert!(named(harness.root_id(), "Changescape section overview").is_some());
    assert!(named(harness.root_id(), "Changescape block narration").is_some());
    let rail = named(harness.root_id(), "Changescape rail").expect("no rail");
    assert_eq!(
        child_names(rail),
        [
            "Changescape rail overview",
            "Changescape rail map",
            "Changescape rail behavior",
            "Changescape rail code",
        ]
    );

    // A section with views opens; one with none does not; Code is Code.
    click(&mut harness, "Changescape rail map");
    assert!(named(harness.root_id(), "Changescape section map").is_some());
    click(&mut harness, "Changescape rail behavior");
    assert!(named(harness.root_id(), "Changescape section map").is_some());
    click(&mut harness, "Changescape rail code");
    assert_eq!(state.tab.get(), ItemTab::Code);
}

#[test]
fn a_changescape_of_an_earlier_head_is_shown_as_stale() {
    let root = cache_with("01-minimal", "aaaaaaa1111");
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape rail").is_some());
    assert!(rect(&harness, "Changescape commit bar").height() > 0.0);
    assert!(state.changescape.get().unwrap().stale());
}

#[test]
fn a_changescape_of_another_item_shows_nothing() {
    let root = cache_with("01-minimal", HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    state.changescape.update(|shown| {
        let found = shown.as_ref().unwrap().found.clone();
        *shown = Some(Arc::new(Shown::new("other".into(), found)));
    });
    redraw(&mut harness);
    assert!(named(harness.root_id(), "Changescape pill").is_none());
}

#[test]
fn a_row_with_a_function_opens_code_at_its_first_line() {
    let (mut harness, state, _root) = opened("01-minimal");
    click(&mut harness, "Changescape rail map");
    let row = nth(&harness, "Changescape row", 0);
    click_at(&mut harness, row);
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(state.active.get().as_deref(), Some(OPS));
    assert_eq!(state.jump_caret.get(), Some((0, 0)), "line 1, from 0");
}

#[test]
fn a_removed_function_shows_its_base_excerpt() {
    let root = cache_with_more(HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    click(&mut harness, "Changescape rail map");
    assert!(named(harness.root_id(), "Changescape base popover").is_none());
    let row = nth(&harness, "Changescape row", 1);
    click_at(&mut harness, row);
    assert_eq!(
        state.tab.get(),
        ItemTab::Changescape,
        "nothing on the head to open"
    );
    assert_eq!(
        state.changescape_popover.get().as_deref(),
        Some("fn:calc.ops.old")
    );
    assert!(rect(&harness, "Changescape base popover").height() > 0.0);

    click(&mut harness, "Changescape show in diff");
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(state.layout.get(), crate::review::model::Layout::Split);
    assert_eq!(state.changescape_popover.get(), None);
}

#[test]
fn a_hot_node_in_a_diagram_opens_its_element() {
    let root = cache_with_more(HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    click(&mut harness, "Changescape rail map");
    // Laid out once more, now the block's width has set the zoom.
    redraw(&mut harness);
    let diagram = nth(&harness, "Changescape diagram", 0);
    let z = diagram.width() / 200.0;
    assert!(z > 0.5, "fitted to the block, not left tiny: {z}");
    let (x, y) = (diagram.x0 + 40.0 * z, diagram.y0 + 20.0 * z);
    harness.pointer_move(x, y);
    harness.click(x, y);
    redraw(&mut harness);
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(state.active.get().as_deref(), Some(OPS));
    assert_eq!(state.jump_caret.get(), Some((0, 0)));
}

/// The window on `cache_with_more`'s Map, laid out once more so that
/// the diagram fits its block.
fn on_map() -> (HeadlessHarness, ReviewState, tempfile::TempDir) {
    let root = cache_with_more(HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    click(&mut harness, "Changescape rail map");
    redraw(&mut harness);
    (harness, state, root)
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
fn expand_draws_the_diagram_larger_beside_the_rail() {
    let (mut harness, state, _root) = on_map();
    let small = nth(&harness, "Changescape diagram", 0);
    click(&mut harness, "Changescape expand");
    redraw(&mut harness);
    assert_eq!(
        state.changescape_expanded.get(),
        Some(("diagram".to_string(), 0))
    );
    let overlay = rect(&harness, "Changescape expanded diagram");
    let rail = rect(&harness, "Changescape rail");
    let bar = rect(&harness, "Changescape commit bar");
    assert!(overlay.x0 >= rail.x1 - 0.5, "the rail stays in sight");
    assert!(overlay.y0 >= bar.y1 - 0.5, "the commit bar stays in sight");
    let large = expanded_diagram(&harness);
    assert!(
        large.width() > small.width(),
        "{} is not larger than {}",
        large.width(),
        small.width()
    );
    // Fitted to the height as well as the width.
    assert!(large.height() <= overlay.height());
    assert!(overlay.contains(large.center()));
}

#[test]
fn escape_and_collapse_put_an_expanded_diagram_away() {
    use floem::prelude::{Code, Key, KeyState, KeyboardEvent, Modifiers, NamedKey};
    let (mut harness, state, _root) = on_map();
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
fn a_hot_node_in_an_expanded_diagram_opens_code_and_closes_it() {
    let (mut harness, state, _root) = on_map();
    click(&mut harness, "Changescape expand");
    redraw(&mut harness);
    let diagram = expanded_diagram(&harness);
    let z = diagram.width() / 200.0;
    let (x, y) = (diagram.x0 + 40.0 * z, diagram.y0 + 20.0 * z);
    harness.pointer_move(x, y);
    harness.click(x, y);
    redraw(&mut harness);
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(state.active.get().as_deref(), Some(OPS));
    assert_eq!(state.changescape_expanded.get(), None);
}

/// A review thread's first comment on `line` of `calc/ops.py`, on `side`.
fn comment(id: u64, line: u64, side: &str) -> perch_github::ReviewComment {
    serde_json::from_value(serde_json::json!({
        "id": id, "pull_request_review_id": 1, "in_reply_to_id": null,
        "user": "u", "body": "why?", "path": OPS,
        "line": line, "original_line": line, "diff_hunk": "", "side": side,
        "created_at": "2026-10-07T12:00:00Z", "updated_at": null, "html_url": ""
    }))
    .unwrap()
}

fn with_threads(state: ReviewState, comments: Vec<perch_github::ReviewComment>) {
    state
        .discussion
        .set(Some(Arc::new(perch_github::Discussion {
            review_comments: comments,
            ..Default::default()
        })));
}

#[test]
fn a_thread_in_an_elements_lines_shows_a_badge_of_one() {
    let root = cache_with_more(HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    // In `add()`, and on a line no element holds.
    with_threads(state, vec![comment(1, 2, "right"), comment(2, 9, "right")]);
    click(&mut harness, "Changescape pill");
    click(&mut harness, "Changescape rail map");
    let mut badges = Vec::new();
    all_named(harness.root_id(), "Changescape thread badge", &mut badges);
    let shown: Vec<f64> = badges
        .iter()
        .map(|b| b.get_visual_rect_no_clip().width())
        .collect();
    // Two rows, then the diagram's one hot node.
    assert_eq!(shown.len(), 3, "one slot per row and node with an element");
    assert!(shown[0] > 0.0, "the first row's function holds the thread");
    assert_eq!(shown[1], 0.0, "old() holds none");

    // The badge goes to the thread.
    let first = badges[0].get_visual_rect_no_clip();
    click_at(&mut harness, first);
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(state.jump_caret.get(), Some((1, 0)));
}

#[test]
fn a_resolved_thread_and_a_reply_do_not_count() {
    let root = cache_with_more(HEAD);
    let found = crate::backend::changescape::find(root.path(), REPO, 812, HEAD);
    let page = found.page.clone().unwrap();
    let index = perch_changescape::LineIndex::new(&page);
    let mut reply = comment(3, 1, "right");
    reply.in_reply_to_id = Some(1);
    let discussion = perch_github::Discussion {
        review_comments: vec![comment(1, 1, "right"), reply, comment(2, 5, "left")],
        review_threads: vec![perch_github::ReviewThreadState {
            node_id: String::new(),
            root_comment_id: 1,
            resolved: true,
            outdated: false,
            can_resolve: false,
        }],
        ..Default::default()
    };
    let counts = crate::review::changescape::threads::count(&index, &discussion);
    assert_eq!(counts.0.len(), 1);
    let (n, anchor) = counts.of("fn:calc.ops.old").unwrap();
    assert_eq!(
        (*n, anchor.line),
        (1, 5),
        "a base-side thread on the removed function"
    );
}

#[test]
fn the_caret_in_an_element_shows_the_code_strip_which_leads_back() {
    let root = cache_with("01-minimal", HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    let text: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    let file = crate::backend::build_loaded_file(
        OPS.into(),
        text,
        String::new(),
        None,
        false,
        perch_git::Scope::Branch,
        String::new(),
    );
    state.scope.set(perch_git::Scope::Branch);
    state.files.update(|files| {
        files.insert(OPS.to_string(), Arc::new(file));
    });
    state.active.set(Some(OPS.to_string()));
    state.show_tab(ItemTab::Code);
    // Drawn once first: opening the file puts the caret at its top.
    redraw(&mut harness);
    // Outside every element: no strip.
    state.cursor_line.set(9);
    redraw(&mut harness);
    assert!(named(harness.root_id(), "Changescape strip link").is_none());
    // Line 1, in `add()`.
    state.cursor_line.set(0);
    redraw(&mut harness);
    assert!(rect(&harness, "Changescape code strip").height() > 0.0);
    click(&mut harness, "Changescape strip link");
    assert_eq!(state.tab.get(), ItemTab::Changescape);
    assert_eq!(
        state.changescape_section.get().as_deref(),
        Some("map"),
        "the first section that shows it: Placement's table"
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

/// What the backend sends for a build in `json`, with an earlier head's
/// page when `page`, and `can_build`.
fn shown_build(state: ReviewState, json: &str, page: bool, can_build: bool) -> tempfile::TempDir {
    let root = if page {
        cache_with("01-minimal", "aaaaaaa1111")
    } else {
        tempfile::tempdir().unwrap()
    };
    let dir = perch_changescape::build_dir(root.path(), REPO, 812, HEAD);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("status.json"), json).unwrap();
    let mut found = crate::backend::changescape::find(root.path(), REPO, 812, HEAD);
    found.can_build = can_build;
    crate::review::changescape::set_can_build(can_build);
    state
        .changescape
        .set(Some(Arc::new(Shown::new("pr".into(), Arc::new(found)))));
    root
}

#[test]
fn a_first_build_running_shows_the_pill_and_the_building_view() {
    let (mut harness, state) = harness_with_cache(None);
    let _root = shown_build(state, &status_json("running", None), false, true);
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape building").is_some());
    assert!(named(harness.root_id(), "Changescape rail").is_none());
}

#[test]
fn a_build_over_an_earlier_page_keeps_the_page_and_says_so() {
    let (mut harness, state) = harness_with_cache(None);
    let _root = shown_build(state, &status_json("running", None), true, true);
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape rail").is_some());
    assert!(rect(&harness, "Changescape commit bar").height() > 0.0);
    assert!(state.changescape.get().unwrap().stale());
    assert_eq!(
        rect(&harness, "Changescape rebuild").width(),
        0.0,
        "no second build while one runs"
    );
}

#[test]
fn a_failed_build_offers_retry_only_where_a_build_can_run() {
    let failed = status_json("failed", Some("uv: command not found"));
    let (mut harness, state) = harness_with_cache(None);
    let _root = shown_build(state, &failed, false, true);
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape failed").is_some());
    assert!(rect(&harness, "Changescape retry").width() > 0.0);

    let (mut harness, state) = harness_with_cache(None);
    let _root = shown_build(state, &failed, false, false);
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

/// The tab open on case `name`, with `can_build`.
fn refused(name: &str, can_build: bool) -> (HeadlessHarness, ReviewState, tempfile::TempDir) {
    crate::review::changescape::set_can_build(can_build);
    opened(name)
}

#[test]
fn case_02_offers_rebuild_and_keeps_the_pill() {
    let (harness, state, _root) = refused("02-status-without-page-schema", true);
    assert!(matches!(
        state.changescape.get().unwrap().lookup(),
        perch_changescape::Lookup::Outdated(_)
    ));
    assert_eq!(state.tab.get(), ItemTab::Changescape);
    assert!(named(harness.root_id(), "Changescape refused").is_some());
    assert!(named(harness.root_id(), "Changescape rail").is_none());
    assert!(rect(&harness, "Changescape rebuild").width() > 0.0);
    // Where no build can run, the browser page.
    let (harness, _, _root) = refused("02-status-without-page-schema", false);
    assert!(named(harness.root_id(), "Changescape open page").is_some());
}

#[test]
fn case_03_offers_the_browser_page() {
    let (harness, state, _root) = refused("03-page-schema-4", true);
    assert!(matches!(
        state.changescape.get().unwrap().lookup(),
        perch_changescape::Lookup::TooNew(_)
    ));
    assert!(named(harness.root_id(), "Changescape refused").is_some());
    assert!(named(harness.root_id(), "Changescape open page").is_some());
    assert!(named(harness.root_id(), "Changescape rebuild").is_none());
}

#[test]
fn a_page_whose_status_misstates_its_version_is_refused_by_the_page() {
    let root = cache_with("03-page-schema-4", HEAD);
    let dir = perch_changescape::build_dir(root.path(), REPO, 812, HEAD);
    let status = std::fs::read_to_string(dir.join("status.json")).unwrap();
    std::fs::write(
        dir.join("status.json"),
        status.replace(r#""page_schema": 4"#, r#""page_schema": 3"#),
    )
    .unwrap();
    let found = crate::backend::changescape::find(root.path(), REPO, 812, HEAD);
    assert!(found.page.is_none());
    assert!(matches!(found.lookup, perch_changescape::Lookup::TooNew(_)));
}

#[test]
fn a_page_that_does_not_read_shows_why_and_the_browser_page() {
    crate::review::changescape::set_can_build(false);
    let root = cache_patched("01-minimal", HEAD, |page| {
        page["elements"]["fn:calc.ops.add"]["st"] = "vanished".into();
    });
    let (mut harness, _state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape failed").is_some());
    assert!(rect(&harness, "Changescape open page").width() > 0.0);
}

#[test]
fn cases_10_and_11_draw_the_browser_line_for_a_block_and_the_rest() {
    for case in ["10-unknown-block-type", "11-malformed-block"] {
        let (mut harness, _, _root) = opened(case);
        click(&mut harness, "Changescape rail map");
        let mut lines = Vec::new();
        all_named(harness.root_id(), "Changescape open page", &mut lines);
        assert_eq!(lines.len(), 1, "{case}: one block in the browser page");
        assert!(named(harness.root_id(), "Changescape row fn:calc.ops.add").is_some());
    }
}

/// The lines of the Placement table, in order: its rows, its section
/// headers and its fold controls, by debug name.
fn table_lines(harness: &HeadlessHarness) -> Vec<String> {
    fn walk(id: floem::ViewId, out: &mut Vec<String>) {
        let name = id.debug_name();
        let name = name.split(" - ").next().unwrap_or_default().trim_end();
        if name.starts_with("Changescape row")
            || name.starts_with("Changescape group")
            || name.starts_with("Changescape fold")
        {
            out.push(name.trim_start_matches("Changescape ").to_string());
            return;
        }
        for child in id.children() {
            walk(child, out);
        }
    }
    let view = named(harness.root_id(), "Changescape view placement").expect("no Placement");
    let mut out = Vec::new();
    walk(view, &mut out);
    out
}

/// The tab on case `name`'s Placement table.
fn placement(name: &str) -> (HeadlessHarness, ReviewState, tempfile::TempDir) {
    let (mut harness, state, root) = opened(name);
    click(&mut harness, "Changescape rail map");
    (harness, state, root)
}

#[test]
fn case_04_marks_the_key_row_and_folds_nothing() {
    let (harness, _, _root) = placement("04-unknown-rank");
    assert_eq!(
        table_lines(&harness),
        [
            "row key fn:calc.ops.add",
            "row fn:calc.ops.sub",
            "row fn:calc.ops.mul"
        ]
    );
}

#[test]
fn case_05_draws_rows_with_no_group_first() {
    let (harness, _, _root) = placement("05-group-not-in-groups");
    assert_eq!(
        table_lines(&harness),
        [
            "row fn:calc.ops.sub",
            "group calc.ops",
            "row fn:calc.ops.add",
            "row fn:calc.ops.mul"
        ]
    );
}

#[test]
fn case_06_a_closed_group_opens_to_its_fold() {
    let (mut harness, _, _root) = placement("06-groups-closed");
    assert_eq!(
        table_lines(&harness),
        ["group calc.ops", "row fn:calc.ops.add", "group calc.tests"]
    );
    click(&mut harness, "Changescape group calc.tests");
    assert_eq!(
        table_lines(&harness)[2..],
        ["group calc.tests", "fold: Show 2 more"]
    );
    click(&mut harness, "Changescape fold");
    assert_eq!(
        table_lines(&harness)[3..],
        [
            "row fn:calc.ops.fixture",
            "row fn:calc.ops.setup",
            "fold: Show fewer"
        ]
    );
}

#[test]
fn case_06_the_selected_elements_row_shows_and_its_section_opens() {
    let (mut harness, state, _root) = placement("06-groups-closed");
    state
        .changescape_popover
        .set(Some("fn:calc.ops.fixture".into()));
    redraw(&mut harness);
    assert_eq!(
        table_lines(&harness)[2..],
        [
            "group calc.tests",
            "row fn:calc.ops.fixture",
            "fold: Show 1 more"
        ]
    );
}

#[test]
fn case_07_one_control_holds_the_rest_in_order() {
    let (mut harness, _, _root) = placement("07-mixed-fold");
    assert_eq!(
        table_lines(&harness),
        [
            "row fn:calc.ops.add",
            "row fn:calc.ops.sub",
            "fold: Show 4 more (2 minor: small helpers)"
        ]
    );
    click(&mut harness, "Changescape fold");
    assert_eq!(
        table_lines(&harness),
        [
            "row fn:calc.ops.add",
            "row fn:calc.ops.sub",
            "row fn:calc.ops.mul",
            "row fn:calc.ops.div",
            "row fn:calc.ops.clamp",
            "row fn:calc.ops.sign",
            "fold: Show fewer"
        ]
    );
}

#[test]
fn case_08_folds_minor_rows_without_groups() {
    let (harness, _, _root) = placement("08-ranks-without-groups");
    assert_eq!(
        table_lines(&harness),
        [
            "row key fn:calc.ops.add",
            "row fn:calc.ops.sub",
            "fold: Show 1 more"
        ]
    );
}

#[test]
fn case_09_a_facet_dims_rows_outside_it_and_hides_them_on_request() {
    let (mut harness, state, _root) = placement("09-facet-rows-without-el");
    assert_eq!(
        table_lines(&harness),
        ["row fn:calc.ops.add", "row fn:calc.ops.sub", "row"]
    );
    let page = state.changescape.get().unwrap().page().unwrap().clone();
    crate::review::changescape::facet_menu(state, &page)
        .submenu_named("Focus")
        .expect("no Focus")
        .choose("New core");
    assert_eq!(
        state.changescape_facet.get(),
        Some(("focus".to_string(), "core".to_string()))
    );
    redraw(&mut harness);
    assert_eq!(
        table_lines(&harness),
        ["row fn:calc.ops.add", "row dim fn:calc.ops.sub", "row"],
        "a row without an element stays as it is"
    );
    assert!(named(harness.root_id(), "Changescape facet note").is_some());

    click(&mut harness, "Changescape facet hide");
    assert_eq!(table_lines(&harness), ["row fn:calc.ops.add", "row"]);

    click(&mut harness, "Changescape whole change");
    assert_eq!(state.changescape_facet.get(), None);
    assert!(named(harness.root_id(), "Changescape facet note").is_none());
    assert_eq!(table_lines(&harness).len(), 3);
}

#[test]
fn only_a_page_with_facets_has_a_picker() {
    let (harness, _, _root) = opened("01-minimal");
    assert!(named(harness.root_id(), "Changescape facet picker").is_none());
    let (harness, _, _root) = opened("09-facet-rows-without-el");
    assert!(named(harness.root_id(), "Changescape facet picker").is_some());
}

/// The minimal case at stage `stage` without Placement's caption, as the
/// backend sends it with `building`.
fn facts_page(state: ReviewState, stage: &str, building: bool) -> tempfile::TempDir {
    let root = cache_patched("01-minimal", HEAD, |page| {
        for view in page["views"].as_array_mut().unwrap() {
            view["narration"] = serde_json::Value::Null;
        }
    });
    let dir = perch_changescape::build_dir(root.path(), REPO, 812, HEAD);
    let status = std::fs::read_to_string(dir.join("status.json")).unwrap();
    std::fs::write(
        dir.join("status.json"),
        status.replace(r#""stage": "narrated""#, &format!(r#""stage": "{stage}""#)),
    )
    .unwrap();
    let mut found = crate::backend::changescape::find(root.path(), REPO, 812, HEAD);
    found.building = building;
    state
        .changescape
        .set(Some(Arc::new(Shown::new("pr".into(), Arc::new(found)))));
    root
}

#[test]
fn facts_drawn_while_this_perchs_build_runs_say_captions_are_coming() {
    let (mut harness, state) = harness_with_cache(None);
    let _root = facts_page(state, "facts", true);
    redraw(&mut harness);
    click(&mut harness, "Changescape pill");
    click(&mut harness, "Changescape rail map");
    assert!(state.changescape.get().unwrap().captions_pending());
    assert!(rect(&harness, "Changescape caption pending").width() > 0.0);

    // The build ended: the same page, and nothing more is coming.
    let _root = facts_page(state, "facts", false);
    redraw(&mut harness);
    assert!(!state.changescape.get().unwrap().captions_pending());
    assert_eq!(rect(&harness, "Changescape caption pending").width(), 0.0);

    // Narrated while the run goes on: nothing pending either.
    let _root = facts_page(state, "narrated", true);
    redraw(&mut harness);
    assert!(!state.changescape.get().unwrap().captions_pending());
}
