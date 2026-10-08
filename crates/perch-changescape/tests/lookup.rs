//! Lookup and load on the conformance fixtures, copied into a cache root
//! of their own.

use std::path::{Path, PathBuf};

use perch_changescape::{load, lookup, DiffSide, LineIndex, Lookup, State};

const REPO: &str = "calc/calc";
const HEAD: &str = "3e81042d07e4d404b84526777e768a456a6a4fa9";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/conformance")
        .join(name)
}

/// Copy a fixture's build into `root` as pull request 7 at `sha`.
fn install(root: &Path, name: &str, sha: &str) -> PathBuf {
    let to = perch_changescape::build_dir(root, REPO, 7, sha);
    std::fs::create_dir_all(&to).unwrap();
    for file in ["page.json", "status.json"] {
        std::fs::copy(fixture(name).join(file), to.join(file)).unwrap();
    }
    to
}

/// Write a status of `state`, finished at `finished`, of page format
/// `page_schema`.
fn status(dir: &Path, state: &str, finished: Option<&str>, page_schema: Option<u32>) {
    let mut json = serde_json::json!({
        "schema": 2, "state": state, "started": "2026-10-08T12:00:00Z",
        "finished": finished, "stage": "narrated", "error": null, "skill_version": "3.0.0",
    });
    if let Some(n) = page_schema {
        json["page_schema"] = n.into();
    }
    if state == "failed" {
        json["error"] = "boom".into();
    }
    std::fs::write(dir.join("status.json"), json.to_string()).unwrap();
}

#[test]
fn no_cache_root_is_no_map() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("absent");
    assert_eq!(lookup(&missing, REPO, 7, HEAD), Lookup::None);
}

#[test]
fn a_build_of_the_head_is_current() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), "01-minimal", HEAD);
    let Lookup::Current(build) = lookup(root.path(), REPO, 7, HEAD) else {
        panic!("expected a current map");
    };
    assert_eq!(build.status.state, State::Done);
    assert_eq!(build.short_sha(), "3e81042");
}

#[test]
fn a_build_of_another_head_is_stale_and_the_newest_wins() {
    let root = tempfile::tempdir().unwrap();
    let older = install(root.path(), "01-minimal", "aaaaaaa1");
    status(&older, "done", Some("2026-10-01T00:00:00Z"), Some(3));
    let newer = install(root.path(), "01-minimal", "bbbbbbb2");
    status(&newer, "done", Some("2026-10-05T00:00:00Z"), Some(3));
    let failed = install(root.path(), "01-minimal", "ccccccc3");
    status(&failed, "failed", Some("2026-10-06T00:00:00Z"), Some(3));
    let Lookup::Stale(build) = lookup(root.path(), REPO, 7, HEAD) else {
        panic!("expected a stale map");
    };
    assert_eq!(build.head_sha, "bbbbbbb2");
}

#[test]
fn a_running_build_keeps_the_earlier_map() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), "01-minimal", "aaaaaaa1");
    let running = perch_changescape::build_dir(root.path(), REPO, 7, HEAD);
    std::fs::create_dir_all(&running).unwrap();
    status(&running, "running", None, None);
    let lookup = lookup(root.path(), REPO, 7, HEAD);
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
    let dir = perch_changescape::build_dir(root.path(), REPO, 7, HEAD);
    std::fs::create_dir_all(&dir).unwrap();
    status(&dir, "failed", None, None);
    let Lookup::Failed(build) = lookup(root.path(), REPO, 7, HEAD) else {
        panic!("expected a failed build");
    };
    assert_eq!(build.status.error.as_deref(), Some("boom"));
}

#[test]
fn a_status_that_does_not_match_its_schema_is_a_failed_build() {
    let root = tempfile::tempdir().unwrap();
    let dir = install(root.path(), "01-minimal", HEAD);
    std::fs::write(dir.join("status.json"), r#"{ "state": "done" }"#).unwrap();
    let Lookup::Failed(build) = lookup(root.path(), REPO, 7, HEAD) else {
        panic!("expected a failed build");
    };
    assert!(build
        .status
        .error
        .unwrap()
        .starts_with("status.json could not be read"));
}

#[test]
fn a_page_with_no_status_is_of_an_older_version() {
    let root = tempfile::tempdir().unwrap();
    let dir = install(root.path(), "01-minimal", HEAD);
    std::fs::remove_file(dir.join("status.json")).unwrap();
    assert!(matches!(
        lookup(root.path(), REPO, 7, HEAD),
        Lookup::Outdated(_)
    ));
}

#[test]
fn an_earlier_head_of_an_older_version_stands_in_for_nothing() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), "02-status-without-page-schema", "aaaaaaa1");
    let found = lookup(root.path(), REPO, 7, HEAD);
    assert!(
        matches!(&found, Lookup::Outdated(b) if b.head_sha == "aaaaaaa1"),
        "{found:?}"
    );
    // A drawable one is the stand-in.
    let drawn = install(root.path(), "01-minimal", "bbbbbbb2");
    let found = lookup(root.path(), REPO, 7, HEAD);
    assert!(
        matches!(&found, Lookup::Stale(b) if b.head_sha == "bbbbbbb2"),
        "{found:?}"
    );
    // A running build does not show the outdated one meanwhile.
    std::fs::remove_dir_all(drawn).unwrap();
    let running = perch_changescape::build_dir(root.path(), REPO, 7, HEAD);
    std::fs::create_dir_all(&running).unwrap();
    status(&running, "running", None, None);
    assert!(matches!(
        lookup(root.path(), REPO, 7, HEAD),
        Lookup::Running(_, None)
    ));
}

#[test]
fn a_page_whose_status_misstates_its_version_is_refused_when_read() {
    let root = tempfile::tempdir().unwrap();
    let dir = install(root.path(), "03-page-schema-4", HEAD);
    status(&dir, "done", Some("2026-10-08T12:01:00Z"), Some(3));
    let found = lookup(root.path(), REPO, 7, HEAD);
    let error = load(found.drawable().unwrap()).unwrap_err();
    assert!(matches!(
        error,
        perch_changescape::Error::Version { schema: 4, .. }
    ));
    assert!(matches!(found.with_page_schema(4), Lookup::TooNew(_)));
}

#[test]
fn the_line_index_finds_the_narrowest_element() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), "01-minimal", HEAD);
    let lookup = lookup(root.path(), REPO, 7, HEAD);
    let page = load(lookup.drawable().unwrap()).unwrap();
    let index = LineIndex::new(&page);
    // The module is the whole file, and adds no range.
    assert_eq!(
        index.at(DiffSide::Head, "calc/ops.py", 1),
        ["fn:calc.ops.add"]
    );
    assert_eq!(
        index.innermost(DiffSide::Head, "calc/ops.py", 2),
        Some("fn:calc.ops.add")
    );
    assert!(index.at(DiffSide::Head, "calc/ops.py", 3).is_empty());
    assert!(index.at(DiffSide::Base, "calc/ops.py", 1).is_empty());
}
