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
    let root = tempfile::tempdir().unwrap();
    let to = perch_changescape::build_dir(root.path(), REPO, 812, sha);
    std::fs::create_dir_all(&to).unwrap();
    for entry in std::fs::read_dir(fixtures().join("dblect-356").join(HEAD)).unwrap() {
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
