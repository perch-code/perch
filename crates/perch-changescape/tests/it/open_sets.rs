//! A page with values the schema leaves open, and the fields it added
//! later, still reads. Case 12 of the conformance fixtures, changed.

use perch_changescape::page::{picture_kind, PictureKind};
use perch_changescape::{load, lookup};

use crate::{install_patched, HEAD, REPO};

#[test]
fn unknown_values_and_new_fields_read_as_the_schema_says() {
    let root = tempfile::tempdir().unwrap();
    install_patched(root.path(), "12-tour", HEAD, |page| {
        page["elements"]["fn:calc.ops.multiply"]["st"] = "split".into();
        let file = &mut page["files"][0];
        file["kind"] = "lockfile".into();
        file["vendored_by"] = "submodule".into();
        page["comments"] = serde_json::json!([{
            "id": 1, "kind": "commit", "by": "a", "at": "2026-01-01T00:00:00Z", "body": "b",
            "url": "u", "reply_to": null, "path": null, "side": null, "line": null,
            "start": null, "outdated": false, "state": null,
        }]);
        let stop = &mut page["tour"]["chapters"][0]["stops"][0];
        stop["pictures"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "kind": "table", "rows": [1, 2] }));
        stop["glance"] = "a field from a later version".into();
        // A block of a shape no version has: the views' blocks are not read.
        page["views"][0]["blocks"] = serde_json::json!([{ "type": "table", "rows": "x" }]);
    });
    let found = lookup(root.path(), REPO, 7, HEAD);
    let page = load(found.drawable().expect("does not draw")).expect("does not read");

    assert_eq!(page.elements["fn:calc.ops.multiply"].st.as_str(), "split");
    assert_eq!(page.files[0].kind.as_str(), "lockfile");
    let pictures = &page.tour().expect("the tour does not read").chapters[0].stops[0].pictures;
    assert_eq!(picture_kind(&pictures[1].kind), PictureKind::Other);
}
