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
        ["01-minimal", "12-tour"],
        "a new case needs a test here"
    );
}

#[test]
fn case_01_draws_without_a_tour() {
    let page = page("01-minimal");
    assert!(page.tour().is_none());
    let add = &page.elements["fn:calc.ops.add"];
    assert_eq!(add.kind, perch_changescape::v4::ElementKind::Function);
    assert_eq!(add.name, "calc.ops.add");
}

#[test]
fn case_12_reads_the_tour() {
    let page = page("12-tour");
    let tour = page.tour().expect("no tour");
    assert!(tour.narrated);
    assert_eq!(tour.chapters.len(), 1);
    let (in_chapter, stop) = page.stop("multiply-helper").expect("no stop");
    assert_eq!(in_chapter.id, "change");
    let (file, windows) = perch_changescape::page::hunk(&stop.pictures[0]).unwrap();
    assert_eq!(file, "calc/ops.py");
    assert_eq!(windows[0].head, Some((4, 5)));
    assert_eq!(windows[0].base, None);
}
