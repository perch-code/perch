//! Lookup and load on the fixtures, copied into a cache root of their own.

use std::path::{Path, PathBuf};

use perch_changescape::page::BlockBody;
use perch_changescape::{count_by_element, load, lookup, DiffSide, LineIndex, Lookup, State};

const REPO: &str = "dvryaboy/dblect";
const HEAD: &str = "3e81042d07e4d404b84526777e768a456a6a4fa9";
const UNKNOWN: &str = "1111111111111111111111111111111111111111";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Copy a fixture's build folder into `root` as pull request 356 at `sha`.
fn install(root: &Path, fixture_dir: &Path, sha: &str) -> PathBuf {
    let to = perch_changescape::build_dir(root, REPO, 356, sha);
    std::fs::create_dir_all(&to).unwrap();
    for entry in std::fs::read_dir(fixture_dir).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
    to
}

fn status(dir: &Path, json: &str) {
    std::fs::write(dir.join("status.json"), json).unwrap();
}

#[test]
fn no_cache_root_is_no_map() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("absent");
    assert_eq!(lookup(&missing, REPO, 356, HEAD), Lookup::None);
}

#[test]
fn a_build_of_the_head_is_current() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), &fixture("dblect-356").join(HEAD), HEAD);
    let Lookup::Current(build) = lookup(root.path(), REPO, 356, HEAD) else {
        panic!("expected a current map");
    };
    assert_eq!(build.status.state, State::Done);
    assert_eq!(build.short_sha(), "3e81042");
}

#[test]
fn a_build_of_another_head_is_stale_and_the_newest_wins() {
    let root = tempfile::tempdir().unwrap();
    let older = install(root.path(), &fixture("dblect-356").join(HEAD), "aaaaaaa1");
    status(
        &older,
        r#"{ "state": "done", "finished": "2026-10-01T00:00:00Z" }"#,
    );
    let newer = install(root.path(), &fixture("dblect-356").join(HEAD), "bbbbbbb2");
    status(
        &newer,
        r#"{ "state": "done", "finished": "2026-10-05T00:00:00Z" }"#,
    );
    let failed = install(root.path(), &fixture("dblect-356").join(HEAD), "ccccccc3");
    status(
        &failed,
        r#"{ "state": "failed", "finished": "2026-10-06T00:00:00Z", "error": "boom" }"#,
    );
    let Lookup::Stale(build) = lookup(root.path(), REPO, 356, HEAD) else {
        panic!("expected a stale map");
    };
    assert_eq!(build.head_sha, "bbbbbbb2");
}

#[test]
fn a_running_build_keeps_the_earlier_map() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), &fixture("dblect-356").join(HEAD), "aaaaaaa1");
    let running = perch_changescape::build_dir(root.path(), REPO, 356, HEAD);
    std::fs::create_dir_all(&running).unwrap();
    status(
        &running,
        r#"{ "schema": 2, "state": "running", "stage": "facts" }"#,
    );
    let lookup = lookup(root.path(), REPO, 356, HEAD);
    let Lookup::Running(build, Some(previous)) = &lookup else {
        panic!("expected a running build over an earlier map, got {lookup:?}");
    };
    assert_eq!(build.head_sha, HEAD);
    assert_eq!(previous.head_sha, "aaaaaaa1");
    assert_eq!(
        lookup.drawable().map(|b| b.head_sha.as_str()),
        Some("aaaaaaa1")
    );
}

#[test]
fn a_failed_build_with_nothing_earlier_is_failed() {
    let root = tempfile::tempdir().unwrap();
    let dir = perch_changescape::build_dir(root.path(), REPO, 356, HEAD);
    std::fs::create_dir_all(&dir).unwrap();
    status(&dir, r#"{ "state": "failed", "error": "uv not found" }"#);
    let Lookup::Failed(build) = lookup(root.path(), REPO, 356, HEAD) else {
        panic!("expected a failed build");
    };
    assert_eq!(build.status.error.as_deref(), Some("uv not found"));
}

#[test]
fn a_page_with_no_status_is_done() {
    let root = tempfile::tempdir().unwrap();
    let dir = install(root.path(), &fixture("dblect-356").join(HEAD), HEAD);
    std::fs::remove_file(dir.join("status.json")).unwrap();
    assert!(matches!(
        lookup(root.path(), REPO, 356, HEAD),
        Lookup::Current(_)
    ));
}

#[test]
fn the_fixture_loads_with_every_section_in_order() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), &fixture("dblect-356").join(HEAD), HEAD);
    let lookup = lookup(root.path(), REPO, 356, HEAD);
    let page = load(lookup.drawable().unwrap()).unwrap();
    let ids: Vec<&str> = page.sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "overview",
            "context",
            "map",
            "behavior",
            "responsibilities",
            "contracts",
            "surroundings",
            "code"
        ]
    );
    let surroundings = &page.sections[6];
    assert!(page.section_views(surroundings).is_empty());
    assert_eq!(surroundings.reason, "No CI or packaging changes.");
    let behavior = &page.sections[3];
    assert_eq!(page.section_views(behavior)[0].id, "decisions");
    assert_eq!(
        page.first_section_showing("cls:dblect.uniqueness.detector._UncoveredOrder")
            .map(|s| s.id.as_str()),
        Some("map")
    );
}

#[test]
fn unknown_blocks_and_fields_still_load() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), &fixture("unknown").join(UNKNOWN), UNKNOWN);
    let lookup = lookup(root.path(), REPO, 356, UNKNOWN);
    let build = lookup.drawable().unwrap();
    assert_eq!(build.status.stage.as_deref(), Some("facts"));
    let page = load(build).unwrap();
    let overview = &page.views[0];
    let last = overview.blocks.last().unwrap();
    assert_eq!(last.kind, "hologram");
    assert!(matches!(last.body, BlockBody::Other));
    // A title that is a number reads as text.
    assert_eq!(page.views.len(), 5);
}

#[test]
fn the_line_index_finds_the_narrowest_element_and_counts_threads() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), &fixture("dblect-356").join(HEAD), HEAD);
    let lookup = lookup(root.path(), REPO, 356, HEAD);
    let page = load(lookup.drawable().unwrap()).unwrap();
    let index = LineIndex::new(&page);
    let detector = "src/dblect/uniqueness/detector.py";
    assert_eq!(
        index.innermost(DiffSide::Head, detector, 1220),
        Some("fn:dblect.uniqueness.detector._uncovered_order_keys")
    );
    // A whole-file ref adds no range.
    assert_eq!(index.innermost(DiffSide::Head, detector, 10), None);
    // A removed function is on the base side only.
    assert_eq!(
        index.innermost(DiffSide::Base, detector, 1182),
        Some("fn:dblect.uniqueness.detector._order_is_bare")
    );
    let counts = count_by_element(
        &index,
        [
            (DiffSide::Head, detector, 1215),
            (DiffSide::Head, detector, 5),
        ],
    );
    assert_eq!(
        counts.get("fn:dblect.uniqueness.detector._uncovered_order_keys"),
        Some(&1)
    );
    assert_eq!(counts.len(), 1);
}
