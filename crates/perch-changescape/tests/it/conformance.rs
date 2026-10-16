//! Every case of the conformance fixtures, read as Perch reads a build,
//! and checked against what the case's `expect.md` says a reader shows.
//! Cases 01 to 07 are valid; 08 and 09 are refused; the rest load with
//! what they cannot read left out.

use perch_changescape::page::{self, Comment, Target};
use perch_changescape::v4::{self, BlocksItem, CommentKind, CommentRef, Rank, ViewId};
use perch_changescape::{load, lookup, Error, LoadedPage, Lookup, OTHER_FORMAT};

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

/// The rows of the table blocks of the view `id`, in order.
fn rows(page: &LoadedPage, id: ViewId) -> Vec<&v4::TableRow> {
    page.view(id)
        .into_iter()
        .flat_map(|view| &view.blocks)
        .filter_map(|block| match block {
            BlocksItem::Table { rows, .. } => Some(rows),
            _ => None,
        })
        .flatten()
        .collect()
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
            "02-table-groups-fold",
            "03-table-ranks",
            "04-facet-rows-without-element",
            "05-cell-kinds",
            "06-comments",
            "07-tour",
            "08-status-page-schema-3",
            "09-page-schema-3",
            "10-unknown-block-type",
            "11-malformed-table",
            "12-malformed-tour",
            "13-unknown-cell-kind",
            "14-unknown-rank",
        ],
        "a new case needs a test here"
    );
}

#[test]
fn case_01_draws_one_text_and_no_tour() {
    let page = page("01-minimal");
    assert_eq!(page.meta.repo, "acme/widgets");
    assert_eq!(page.meta.number, 7);
    assert!(page.tour().is_none());
    assert!(!page.tour_dropped());
    assert!(page.elements.is_empty());
    let view = page.view(ViewId::Overview).expect("no overview");
    assert!(matches!(
        &view.blocks[..],
        [BlocksItem::Text { text, .. }] if text == "The store gets a cache that drops the oldest key."
    ));
}

#[test]
fn case_02_draws_two_groups_and_the_rows_in_order() {
    let page = page("02-table-groups-fold");
    let table = page
        .view(ViewId::Overview)
        .and_then(|view| {
            view.blocks.iter().find_map(|block| match block {
                BlocksItem::Table {
                    groups, fold, rows, ..
                } => Some((groups, fold, rows)),
                _ => None,
            })
        })
        .expect("no table");
    let (groups, fold, rows) = table;
    assert_eq!(groups.len(), 2);
    assert!(groups[0].open);
    assert!(!groups[1].open);
    assert_eq!(fold.as_ref().and_then(|f| f.show), Some(2));
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].rank, Rank::Key);
    assert_eq!(rows[0].element.as_deref(), Some("fn:pkg.api.handle"));
    assert!(rows[2].dim);
    assert_eq!(rows[2].rank, Rank::Minor);
}

#[test]
fn case_03_draws_the_ranks_in_array_order_without_groups() {
    let page = page("03-table-ranks");
    let ranks: Vec<Rank> = rows(&page, ViewId::Overview)
        .iter()
        .map(|row| row.rank)
        .collect();
    assert_eq!(ranks, [Rank::Key, Rank::Normal, Rank::Minor]);
}

#[test]
fn case_04_draws_rows_that_name_no_element() {
    let page = page("04-facet-rows-without-element");
    let rows = rows(&page, ViewId::Overview);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.element.is_none()));
    assert_eq!(page.facets.len(), 1);
}

#[test]
fn case_05_draws_a_row_with_seven_cells() {
    let page = page("05-cell-kinds");
    let rows = rows(&page, ViewId::Overview);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cells.len(), 7);
}

#[test]
fn case_06_draws_four_comments_in_the_discussion() {
    let page = page("06-comments");
    let kinds: Vec<(CommentKind, i64)> = page
        .comments
        .iter()
        .map(Comment::from)
        .map(|c| (c.kind, c.id))
        .collect();
    assert_eq!(
        kinds,
        [
            (CommentKind::Inline, 101),
            (CommentKind::Inline, 102),
            (CommentKind::Review, 201),
            (CommentKind::Issue, 301),
        ]
    );
    let discussed = page.view(ViewId::Overview).and_then(|view| {
        view.blocks.iter().find_map(|block| match block {
            BlocksItem::Discussion { comments, .. } => Some(comments.len()),
            _ => None,
        })
    });
    assert_eq!(discussed, Some(4));
    assert!(page
        .comment(&CommentRef {
            kind: CommentKind::Review,
            id: 201
        })
        .is_some());
}

#[test]
fn case_07_draws_the_tour_with_its_pictures_flags_and_band() {
    let page = page("07-tour");
    let tour = page.tour().expect("no tour");
    assert_eq!(tour.chapters.len(), 1);
    assert_eq!(tour.chapters[0].title, "The change");
    let (_, stop) = page.stop("put-checks-key").expect("no stop");
    assert_eq!(stop.title, "Store keys must be strings");

    let seq = page::picture_figure(&stop.pictures[0])
        .expect("no seq picture")
        .svg;
    assert_eq!(
        seq.hot.get("n1").map(String::as_str),
        Some("fn:pkg.store.Store.put")
    );
    let (file, windows) = page::hunk(&stop.pictures[1]).expect("no hunk picture");
    assert_eq!(file, "pkg/store.py");
    assert_eq!(windows[0].base, Some((14, 15)));
    assert_eq!(windows[0].head, Some((14, 17)));

    let targets: Vec<Target> = tour
        .flags
        .iter()
        .filter_map(|f| f.target.as_ref().map(Target::from))
        .collect();
    assert_eq!(targets.len(), 3);
    assert_eq!(targets[0], Target::Element("fn:pkg.store.Store.put"));
    assert_eq!(targets[1], Target::View(ViewId::Overview));
    assert!(matches!(
        targets[2],
        Target::Comment(c) if c.id == 101 && c.kind == CommentKind::Inline
    ));

    assert_eq!(tour.below.len(), 1);
    assert_eq!(tour.below[0].label, "Tests");
    assert_eq!(tour.below[0].lines, 5);
    let ending = tour.ending.as_ref().expect("no ending");
    assert_eq!(ending.actions[0].label, "Run the tests");
}

#[test]
fn case_08_is_refused_for_its_status_format() {
    let (found, _root) = found("08-status-page-schema-3");
    assert!(found.drawable().is_none());
    match found {
        Lookup::Failed(build) => assert_eq!(build.status.error.as_deref(), Some(OTHER_FORMAT)),
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn case_09_is_refused_for_its_page_format_before_the_rest_is_read() {
    let (found, _root) = found("09-page-schema-3");
    let build = found.drawable().expect("the build should be found");
    assert!(matches!(load(build), Err(Error::OtherFormat)));
}

#[test]
fn case_10_drops_the_unknown_block_and_keeps_the_table() {
    let page = page("10-unknown-block-type");
    assert_eq!(rows(&page, ViewId::Overview).len(), 4);
    let dropped: Vec<_> = page.dropped_blocks(ViewId::Overview).collect();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].position, 1);
    assert_eq!(dropped[0].tag.as_deref(), Some("heatmap"));
    assert!(!dropped[0].known);
}

#[test]
fn case_11_drops_the_malformed_table() {
    let page = page("11-malformed-table");
    assert!(page.view(ViewId::Overview).unwrap().blocks.is_empty());
    let dropped: Vec<_> = page.dropped_blocks(ViewId::Overview).collect();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].position, 0);
    assert_eq!(dropped[0].tag.as_deref(), Some("table"));
    assert!(dropped[0].known);
}

#[test]
fn case_12_draws_without_the_tour_and_says_it_was_dropped() {
    let page = page("12-malformed-tour");
    assert!(page.tour().is_none());
    assert!(page.tour_dropped());
    assert!(page.view(ViewId::Overview).is_some());
    assert_eq!(page.dropped_blocks(ViewId::Overview).count(), 0);
}

#[test]
fn case_13_drops_the_table_with_the_unknown_cell() {
    let page = page("13-unknown-cell-kind");
    let dropped: Vec<_> = page.dropped_blocks(ViewId::Overview).collect();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].tag.as_deref(), Some("table"));
    assert!(dropped[0].known);
    assert!(page.view(ViewId::Overview).unwrap().blocks.is_empty());
}

#[test]
fn case_14_drops_the_table_with_the_unknown_rank() {
    let page = page("14-unknown-rank");
    let dropped: Vec<_> = page.dropped_blocks(ViewId::Overview).collect();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].tag.as_deref(), Some("table"));
    assert!(dropped[0].known);
    assert!(rows(&page, ViewId::Overview).is_empty());
}
