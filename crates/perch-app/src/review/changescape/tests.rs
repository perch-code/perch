//! The Changescape tab in the whole window, on the fixtures of
//! `perch-changescape`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use floem::headless::HeadlessHarness;
use floem::reactive::{SignalGet, SignalUpdate};

use crate::review::changescape::Shown;
use crate::review::header::tests::{named, pull_request, pull_request_harness_with, rect};
use crate::review::model::{ItemTab, ReviewState};

const REPO: &str = "slatedb/slatedb";
const HEAD: &str = "3e81042d07e4d404b84526777e768a456a6a4fa9";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../perch-changescape/tests/fixtures")
}

/// A cache root with the dblect fixture as pull request 812 at `sha`.
fn cache_with_fixture(sha: &str) -> tempfile::TempDir {
    cache_with("dblect-356", HEAD, sha)
}

/// A cache root with fixture `name`'s build `from` as pull request 812 at
/// `sha`.
fn cache_with(name: &str, from: &str, sha: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let to = perch_changescape::build_dir(root.path(), REPO, 812, sha);
    std::fs::create_dir_all(&to).unwrap();
    for entry in std::fs::read_dir(fixtures().join(name).join(from)).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
    root
}

/// The window on pull request 812 at the fixture's head, with what the
/// backend would find in `root`.
fn harness_with_cache(root: Option<&Path>) -> (HeadlessHarness, ReviewState) {
    let mut pull = pull_request(812);
    pull.head_sha = HEAD.into();
    let (mut harness, _, state) = pull_request_harness_with(pull);
    if let Some(root) = root {
        let found = crate::backend::changescape::find(root, REPO, 812, HEAD);
        state.changescape.set(Some(Arc::new(Shown::new(
            "pr".into(),
            HEAD.into(),
            Arc::new(found),
        ))));
    }
    harness.rebuild();
    harness.paint();
    (harness, state)
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
    harness.rebuild();
    harness.paint();
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
fn a_current_changescape_adds_the_pill_and_opens_on_overview() {
    let root = cache_with_fixture(HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    assert_eq!(
        switch_children(&harness),
        ["Conversation pill", "Changescape pill", "Code pill"]
    );
    click(&mut harness, "Changescape pill");
    assert_eq!(state.tab.get(), ItemTab::Changescape);
    assert!(named(harness.root_id(), "Changescape section overview").is_some());
    let rail = named(harness.root_id(), "Changescape rail").expect("no rail");
    let entries = child_names(rail);
    assert_eq!(
        entries,
        [
            "Changescape rail overview",
            "Changescape rail context",
            "Changescape rail map",
            "Changescape rail behavior",
            "Changescape rail responsibilities",
            "Changescape rail contracts",
            "Changescape rail surroundings",
            "Changescape rail code",
        ]
    );
    assert_eq!(rect(&harness, "Changescape stale banner").height(), 0.0);

    // A section with views opens; one with none does not; Code is Code.
    click(&mut harness, "Changescape rail behavior");
    assert!(named(harness.root_id(), "Changescape section behavior").is_some());
    click(&mut harness, "Changescape rail surroundings");
    assert!(named(harness.root_id(), "Changescape section behavior").is_some());
    click(&mut harness, "Changescape rail code");
    assert_eq!(state.tab.get(), ItemTab::Code);
}

#[test]
fn a_changescape_of_an_earlier_head_shows_the_banner() {
    let root = cache_with_fixture("aaaaaaa1111");
    let (mut harness, _) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    assert!(rect(&harness, "Changescape stale banner").height() > 0.0);
}

#[test]
fn turned_off_there_is_no_pill() {
    let root = cache_with_fixture(HEAD);
    let (harness, state) = harness_with_cache(Some(root.path()));
    // What the backend sends when `enabled = false`.
    state.changescape.set(None);
    let mut harness = harness;
    harness.rebuild();
    harness.paint();
    assert!(named(harness.root_id(), "Changescape pill").is_none());
}

#[test]
fn a_changescape_of_another_item_shows_nothing() {
    let root = cache_with_fixture(HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    state.changescape.update(|shown| {
        let found = shown.as_ref().unwrap().found.clone();
        *shown = Some(Arc::new(Shown::new("other".into(), HEAD.into(), found)));
    });
    harness.rebuild();
    harness.paint();
    assert!(named(harness.root_id(), "Changescape pill").is_none());
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
    harness.rebuild();
    harness.paint();
}

#[test]
fn a_row_with_a_function_opens_code_at_its_first_line() {
    let root = cache_with_fixture(HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    click(&mut harness, "Changescape rail behavior");
    let row = nth(&harness, "Changescape row", 0);
    click_at(&mut harness, row);
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(
        state.active.get().as_deref(),
        Some("src/dblect/uniqueness/detector.py")
    );
    assert_eq!(state.jump_caret.get(), Some((1210, 0)), "line 1211, from 0");
}

#[test]
fn a_removed_function_shows_its_base_excerpt() {
    let root = cache_with_fixture(HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    click(&mut harness, "Changescape rail behavior");
    assert!(named(harness.root_id(), "Changescape base popover").is_none());
    let row = nth(&harness, "Changescape row", 2);
    click_at(&mut harness, row);
    assert_eq!(
        state.tab.get(),
        ItemTab::Changescape,
        "nothing on the head to open"
    );
    assert_eq!(
        state.changescape_popover.get().as_deref(),
        Some("fn:dblect.uniqueness.detector._order_is_bare")
    );
    assert!(rect(&harness, "Changescape base popover").height() > 0.0);

    click(&mut harness, "Changescape show in diff");
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(state.layout.get(), crate::review::model::Layout::Split);
    assert_eq!(state.changescape_popover.get(), None);
}

#[test]
fn an_unknown_block_draws_the_browser_line() {
    let sha = "1111111111111111111111111111111111111111";
    let root = cache_with("unknown", sha, HEAD);
    let (mut harness, _) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    assert!(named(harness.root_id(), "Changescape block hologram").is_some());
    assert!(named(harness.root_id(), "Changescape open page").is_some());
    assert!(named(harness.root_id(), "Changescape block narration").is_some());
}

#[test]
fn a_hot_node_in_a_diagram_opens_its_element() {
    let root = cache_with_fixture(HEAD);
    let (mut harness, state) = harness_with_cache(Some(root.path()));
    click(&mut harness, "Changescape pill");
    click(&mut harness, "Changescape rail map");
    // Laid out once more, now the block's width has set the zoom.
    harness.rebuild();
    harness.paint();
    let diagram = nth(&harness, "Changescape diagram", 0);
    // The fixture's picture is 620 wide; `_uncovered_order_keys` is the
    // box at (40, 60), 120 by 40.
    let z = diagram.width() / 620.0;
    assert!(z > 0.5, "fitted to the block, not left tiny: {z}");
    let (x, y) = (diagram.x0 + 100.0 * z, diagram.y0 + 80.0 * z);
    harness.pointer_move(x, y);
    harness.click(x, y);
    harness.rebuild();
    harness.paint();
    assert_eq!(state.tab.get(), ItemTab::Code);
    assert_eq!(
        state.active.get().as_deref(),
        Some("src/dblect/uniqueness/detector.py")
    );
    assert_eq!(state.jump_caret.get(), Some((1210, 0)));
}
