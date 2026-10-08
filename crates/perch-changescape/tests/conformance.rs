//! Every case of the shared conformance fixtures
//! (`tests/fixtures/conformance/`, copied from the Changescape repo's
//! `skill/schema/fixtures/`), read as Perch reads a build, and checked
//! against what the case's `expect.md` says a reader shows.

use std::path::{Path, PathBuf};

use perch_changescape::fold::{more_label, outside, parts, Part};
use perch_changescape::page::{rank, Rank, Table};
use perch_changescape::{load, lookup, LoadedPage, Lookup};

const REPO: &str = "calc/calc";
const HEAD: &str = "3e81042d07e4d404b84526777e768a456a6a4fa9";

fn cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance")
}

/// The case's build as the lookup finds it, in a cache root of its own.
fn found(name: &str) -> (Lookup, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let to = perch_changescape::build_dir(root.path(), REPO, 7, HEAD);
    std::fs::create_dir_all(&to).unwrap();
    for file in ["page.json", "status.json"] {
        std::fs::copy(cases().join(name).join(file), to.join(file)).unwrap();
    }
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

/// The table of the Placement view: the last block, after any that does
/// not read.
fn placement(page: &LoadedPage) -> Table {
    page.blocks("placement")
        .iter()
        .find_map(|block| block.table())
        .expect("no table in placement")
}

/// The element of each row, by index.
fn els(table: &Table, rows: &[usize]) -> Vec<String> {
    rows.iter()
        .map(|&i| table.rows[i].el.clone().unwrap_or_default())
        .collect()
}

fn folded(table: &Table) -> Vec<Part> {
    parts(table, |_| true, |_| false)
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
            "04-unknown-rank",
            "05-group-not-in-groups",
            "06-groups-closed",
            "07-mixed-fold",
            "08-ranks-without-groups",
            "09-facet-rows-without-el",
            "10-unknown-block-type",
            "11-malformed-block",
        ],
        "a new case needs a test here"
    );
}

#[test]
fn case_01_draws() {
    let page = page("01-minimal");
    let ids: Vec<&str> = page.sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["overview", "map", "behavior", "code"]);
    let behavior = &page.sections[2];
    assert!(page.section_views(behavior).is_empty());
    assert!(behavior.reason.is_some());
    let table = placement(&page);
    assert_eq!(els(&table, &[0]), ["fn:calc.ops.add"]);
    assert_eq!(
        perch_changescape::page::cell(&table.rows[0], "st").text,
        "added"
    );
    assert_eq!(
        page.first_section_showing("fn:calc.ops.add")
            .map(|s| s.id.as_str()),
        Some("map")
    );
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
    assert!(found.refused().unwrap().html_path().ends_with("page.html"));
}

#[test]
fn case_04_an_unknown_rank_is_normal() {
    let table = placement(&page("04-unknown-rank"));
    let ranks: Vec<Rank> = table.rows.iter().map(rank).collect();
    assert_eq!(ranks, [Rank::Key, Rank::Normal, Rank::Normal]);
    assert_eq!(
        table.rows[1].why.as_deref(),
        Some("a rank from a future version")
    );
    let parts = folded(&table);
    assert_eq!(parts.len(), 1);
    assert_eq!(
        els(&table, &parts[0].shown),
        ["fn:calc.ops.add", "fn:calc.ops.sub", "fn:calc.ops.mul"]
    );
    assert!(parts[0].folded.is_empty(), "nothing folds");
}

#[test]
fn case_05_a_group_not_in_groups_is_no_group() {
    let table = placement(&page("05-group-not-in-groups"));
    let parts = folded(&table);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].group, None);
    assert_eq!(els(&table, &parts[0].shown), ["fn:calc.ops.sub"]);
    assert_eq!(parts[1].group, Some(0));
    assert_eq!(table.groups[0].summary.as_deref(), Some("2 functions"));
    assert!(table.groups[0].open);
    assert_eq!(
        els(&table, &parts[1].shown),
        ["fn:calc.ops.add", "fn:calc.ops.mul"]
    );
}

#[test]
fn case_06_a_closed_group_starts_closed_and_a_selected_row_shows() {
    let table = placement(&page("06-groups-closed"));
    assert!(table.groups[0].open);
    assert!(!table.groups[1].open);
    let parts = folded(&table);
    let tests = &parts[1];
    assert!(tests.shown.is_empty());
    assert_eq!(
        els(&table, &tests.folded),
        ["fn:calc.ops.fixture", "fn:calc.ops.setup"]
    );
    assert_eq!(more_label(tests, table.fold.as_ref()), "Show 2 more");
    // The selected element's row shows, and pins its section open.
    let selected = parts_pinning(&table, "fn:calc.ops.fixture");
    assert!(selected[1].pinned);
    assert_eq!(els(&table, &selected[1].shown), ["fn:calc.ops.fixture"]);
}

fn parts_pinning(table: &Table, el: &str) -> Vec<Part> {
    parts(table, |_| true, |row| row.el.as_deref() == Some(el))
}

#[test]
fn case_07_one_control_holds_normal_and_minor_rows() {
    let table = placement(&page("07-mixed-fold"));
    let parts = folded(&table);
    assert_eq!(parts.len(), 1);
    assert_eq!(
        els(&table, &parts[0].shown),
        ["fn:calc.ops.add", "fn:calc.ops.sub"]
    );
    assert_eq!(
        els(&table, &parts[0].folded),
        [
            "fn:calc.ops.mul",
            "fn:calc.ops.div",
            "fn:calc.ops.clamp",
            "fn:calc.ops.sign"
        ],
        "in array order"
    );
    assert_eq!(
        more_label(&parts[0], table.fold.as_ref()),
        "Show 4 more (2 minor: small helpers)"
    );
}

#[test]
fn case_08_ranks_fold_without_groups_or_fold() {
    let table = placement(&page("08-ranks-without-groups"));
    assert!(table.groups.is_empty() && table.fold.is_none());
    let parts = folded(&table);
    assert_eq!(parts.len(), 1, "no sections");
    assert_eq!(rank(&table.rows[parts[0].shown[0]]), Rank::Key);
    assert_eq!(
        els(&table, &parts[0].shown),
        ["fn:calc.ops.add", "fn:calc.ops.sub"]
    );
    assert_eq!(els(&table, &parts[0].folded), ["fn:calc.ops.clamp"]);
    assert_eq!(more_label(&parts[0], None), "Show 1 more");
}

#[test]
fn case_09_a_facet_leaves_rows_without_el_alone() {
    let page = page("09-facet-rows-without-el");
    let table = placement(&page);
    let option = page.facet_option("focus", "core");
    assert_eq!(option.map(|o| o.label.as_str()), Some("New core"));
    let out: Vec<bool> = table.rows.iter().map(|row| outside(option, row)).collect();
    assert_eq!(out, [false, true, false]);
    assert!(table.rows[2].el.is_none());
    let none: Vec<bool> = table.rows.iter().map(|row| outside(None, row)).collect();
    assert_eq!(none, [false, false, false], "without a facet nothing dims");
}

/// The case's Placement blocks, by kind, and whether each read.
fn placement_blocks(name: &str) -> (LoadedPage, Vec<(String, bool)>) {
    let page = page(name);
    let blocks = page
        .blocks("placement")
        .iter()
        .map(|block| {
            let known = matches!(block, perch_changescape::page::PageBlock::Known(_));
            (block.kind().to_string(), known)
        })
        .collect();
    (page, blocks)
}

#[test]
fn case_10_an_unknown_block_type_is_kept_by_name() {
    let (page, blocks) = placement_blocks("10-unknown-block-type");
    assert_eq!(
        blocks,
        [("heatmap".to_string(), false), ("table".to_string(), true)]
    );
    assert_eq!(els(&placement(&page), &[0]), ["fn:calc.ops.add"]);
    assert_eq!(page.blocks("overview")[0].kind(), "narration");
}

#[test]
fn case_11_a_malformed_block_drops_alone() {
    let (page, blocks) = placement_blocks("11-malformed-block");
    assert_eq!(
        blocks,
        [("table".to_string(), false), ("table".to_string(), true)]
    );
    assert_eq!(page.blocks("placement")[1].title(), Some("New functions"));
}
