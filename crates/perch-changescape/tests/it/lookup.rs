//! Lookup and load on the conformance fixtures, copied into a cache root
//! of their own.

use std::path::Path;

use perch_changescape::{load, lookup, lookup_during_run, Lookup, State, StopIndex};

use crate::{install, HEAD, REPO};

/// Write a status of `state`, finished at `finished`.
fn status(dir: &Path, state: &str, finished: Option<&str>) {
    let mut json = serde_json::json!({
        "schema": 2, "state": state, "started": "2026-10-08T12:00:00Z",
        "finished": finished, "stage": "narrated", "error": null, "skill_version": "3.0.0",
    });
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
    status(&older, "done", Some("2026-10-01T00:00:00Z"));
    let newer = install(root.path(), "01-minimal", "bbbbbbb2");
    status(&newer, "done", Some("2026-10-05T00:00:00Z"));
    let failed = install(root.path(), "01-minimal", "ccccccc3");
    status(&failed, "failed", Some("2026-10-06T00:00:00Z"));
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
    status(&running, "running", None);
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
    status(&dir, "failed", None);
    let Lookup::Failed(build) = lookup(root.path(), REPO, 7, HEAD) else {
        panic!("expected a failed build");
    };
    assert_eq!(build.status.error.as_deref(), Some("boom"));
}

#[test]
fn during_a_run_a_failed_build_with_a_page_still_draws_it() {
    let root = tempfile::tempdir().unwrap();
    let dir = install(root.path(), "01-minimal", HEAD);
    status(&dir, "failed", Some("2026-10-08T12:01:00Z"));
    assert!(matches!(
        lookup(root.path(), REPO, 7, HEAD),
        Lookup::Failed(_)
    ));
    let Lookup::Current(build) = lookup_during_run(root.path(), REPO, 7, HEAD) else {
        panic!("expected the page of the step before");
    };
    assert_eq!(build.status.state, State::Done);
    assert_eq!(build.status.error, None);
}

#[test]
fn during_a_run_a_failed_build_with_no_page_is_running_over_the_earlier_map() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), "01-minimal", "aaaaaaa1");
    let dir = perch_changescape::build_dir(root.path(), REPO, 7, HEAD);
    std::fs::create_dir_all(&dir).unwrap();
    status(&dir, "failed", None);
    let lookup = lookup_during_run(root.path(), REPO, 7, HEAD);
    let Lookup::Running(_, Some(previous)) = &lookup else {
        panic!("expected a running build, got {lookup:?}");
    };
    assert_eq!(previous.head_sha, "aaaaaaa1");
}

#[test]
fn during_a_run_a_status_half_written_is_not_the_end() {
    let root = tempfile::tempdir().unwrap();
    let dir = install(root.path(), "01-minimal", HEAD);
    std::fs::write(dir.join("status.json"), r#"{ "state": "do"#).unwrap();
    assert!(lookup_during_run(root.path(), REPO, 7, HEAD).is_running());
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
fn a_page_with_no_status_is_a_finished_build() {
    let root = tempfile::tempdir().unwrap();
    let dir = install(root.path(), "01-minimal", HEAD);
    std::fs::remove_file(dir.join("status.json")).unwrap();
    assert!(matches!(
        lookup(root.path(), REPO, 7, HEAD),
        Lookup::Current(_)
    ));
}

#[test]
fn the_stop_index_finds_the_stop_whose_window_holds_a_line() {
    let root = tempfile::tempdir().unwrap();
    install(root.path(), "12-tour", HEAD);
    let lookup = lookup(root.path(), REPO, 7, HEAD);
    let page = load(lookup.drawable().unwrap()).unwrap();
    let index = StopIndex::new(&page);
    assert_eq!(index.at("calc/ops.py", 1), Some("multiply"));
    assert_eq!(index.at("calc/ops.py", 5), Some("multiply-helper"));
    assert_eq!(index.at("calc/ops.py", 6), None);
    assert_eq!(index.at("calc/other.py", 1), None);
}
