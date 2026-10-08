//! A page with values the schema leaves open, and the fields it added
//! later, still reads. Case 01 of the conformance fixtures, changed.

use std::path::Path;

use perch_changescape::page::{cell, column_kind, comment_kind, file_kind, status, view_state};
use perch_changescape::{load, lookup};

const REPO: &str = "calc/calc";
const HEAD: &str = "3e81042d07e4d404b84526777e768a456a6a4fa9";

#[test]
fn unknown_values_and_new_fields_read_as_the_schema_says() {
    let case = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance/01-minimal");
    let mut page: serde_json::Value =
        serde_json::from_slice(&std::fs::read(case.join("page.json")).unwrap()).unwrap();
    page["elements"]["fn:calc.ops.add"]["st"] = "split".into();
    page["views"][0]["state"] = "partial".into();
    let file = &mut page["files"][0];
    file["kind"] = "lockfile".into();
    file["vendored_by"] = "submodule".into();
    file["vendored_note"] = "Copied from upstream.".into();
    file["lines_by_kind"] = serde_json::json!({ "test": { "add": 1, "del": 0 } });
    page["parts"] = serde_json::json!([{
        "id": "new:calc", "label": "calc", "kind": "new", "verdict": "read",
        "lines": { "add": 2, "del": 0 },
        "files": [{ "path": "calc/ops.py", "add": 2, "del": 0 }],
        "els": ["fn:calc.ops.add"],
    }]);
    page["comments"] = serde_json::json!([{
        "id": 1, "kind": "commit", "by": "a", "at": "2026-01-01T00:00:00Z", "body": "b",
        "url": "u", "reply_to": null, "path": null, "side": null, "line": null,
        "start": null, "outdated": false, "state": null,
    }]);
    let views = page["views"].as_array_mut().unwrap();
    let placement = views.iter_mut().find(|v| v["id"] == "placement").unwrap();
    let table = &mut placement["blocks"][0];
    table["columns"][1]["kind"] = "sparkline".into();
    table["rows"][0]["cells"]["fn"]["facet"] = "part.new".into();

    let root = tempfile::tempdir().unwrap();
    let to = perch_changescape::build_dir(root.path(), REPO, 7, HEAD);
    std::fs::create_dir_all(&to).unwrap();
    std::fs::copy(case.join("status.json"), to.join("status.json")).unwrap();
    std::fs::write(to.join("page.json"), page.to_string()).unwrap();
    let found = lookup(root.path(), REPO, 7, HEAD);
    let page = load(found.drawable().expect("does not draw")).expect("does not read");

    assert_eq!(status(&page.elements["fn:calc.ops.add"].st), "changed");
    assert_eq!(view_state(&page.views[0].state), "na");
    assert_eq!(file_kind(&page.files[0].kind), "other");
    assert_eq!(comment_kind(&page.comments[0].kind), "issue");
    let table = page
        .blocks("placement")
        .iter()
        .find_map(|block| block.table())
        .expect("the table does not read");
    assert_eq!(column_kind(&table.columns[1]), "text");
    assert_eq!(
        cell(&table.rows[0], "fn").facet,
        Some(("part".to_string(), "new".to_string()))
    );
}
