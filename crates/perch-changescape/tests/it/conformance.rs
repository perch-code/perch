//! Every case of the conformance fixtures, read as Perch reads a build,
//! and checked against what the case's `expect.md` says a reader shows.

use perch_changescape::{load, lookup, LoadedPage, Lookup};

use crate::{cases, install, HEAD, REPO};

/// The case's build as the lookup finds it, in a cache root of its own.
fn found(name: &str) -> (Lookup, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), name, HEAD);
    (lookup(root.path(), REPO, 7, HEAD), root)
}

/// The case's page, which must draw.
fn page(name: &str) -> LoadedPage {
    let (found, _root) = found(name);
    let build = found
        .drawable()
        .unwrap_or_else(|| panic!("{name} does not draw: {found:?}"));
    (*load(build).unwrap_or_else(|e| panic!("{name}: {e}"))).clone()
}

#[test]
fn every_case_has_a_test() {
    let mut names: Vec<String> = std::fs::read_dir(cases())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "01-minimal",
            "02-status-without-page-schema",
            "03-page-schema-4",
            "12-tour",
            "13-malformed-tour",
        ],
        "a new case needs a test here"
    );
}

#[test]
fn case_01_draws_without_a_tour() {
    let page = page("01-minimal");
    assert!(page.tour().is_none());
    assert!(page.tour_error().is_none());
    assert!(page.elements.contains_key("fn:calc.ops.add"));
}

#[test]
fn case_02_is_outdated() {
    let (found, _root) = found("02-status-without-page-schema");
    assert!(matches!(found, Lookup::Outdated(_)), "{found:?}");
    assert!(found.drawable().is_none());
}

#[test]
fn case_03_is_too_new() {
    let (found, _root) = found("03-page-schema-4");
    assert!(matches!(found, Lookup::TooNew(_)), "{found:?}");
    assert!(found.refused().is_some());
}

#[test]
fn case_12_reads_the_tour() {
    let page = page("12-tour");
    let tour = page.tour().expect("no tour");
    assert_eq!(tour.narrated, Some(true));
    assert_eq!(tour.chapters.len(), 1);
    let (in_chapter, stop) = page.stop("multiply-helper").expect("no stop");
    assert_eq!(in_chapter.id, "change");
    let (file, windows) = perch_changescape::page::hunk(&stop.pictures[0]).unwrap();
    assert_eq!(file, "calc/ops.py");
    assert_eq!(windows[0].head, Some((4, 5)));
    assert_eq!(windows[0].base, None);
}

#[test]
fn case_13_drops_the_tour_and_keeps_the_page() {
    let page = page("13-malformed-tour");
    assert!(page.tour().is_none());
    assert!(page.tour_error().is_some());
}
