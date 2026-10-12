# Conformance fixtures

These are the cases of page format 4 that Perch reads. Each folder holds a `page.json`, a
`status.json`, and an `expect.md` that says what a correct reader shows, or how it must refuse.

They are copied from the Changescape skill's `contract-fixtures/`, at commit `9439f0f` of the
`page-v4` branch of `/Users/dmitriy/workspace/changescape`. The shapes are in
`../../../schema/page.v4.schema.json` and `../../../schema/status.v4.schema.json`. The meanings
are in `page-data.md` in the Changescape repo, under `skill/src/changescape/contract/`.

| Fixture | Case |
| --- | --- |
| `01-minimal` | A minimal valid page: one view with one text block |
| `02-table-groups-fold` | A table with key, normal, and minor rows, two groups, one closed group, and a fold |
| `03-table-ranks` | A table with ranks, no groups, and no fold |
| `04-facet-rows-without-element` | A facet with options, and table rows with no `element` |
| `05-cell-kinds` | A table row with each cell kind |
| `06-comments` | Each comment kind, and a discussion block |
| `07-tour` | A tour of one chapter and one stop, with pictures, flags, a band, and an ending |
| `08-status-page-schema-3` | `page_schema` 3 in status.json: refused |
| `09-page-schema-3` | `schema` 3 in page.json: refused |
| `10-unknown-block-type` | A block of an unknown `type`: the rest of the view draws |
| `11-malformed-table` | A table with no `columns`: only that block is dropped |
| `12-malformed-tour` | A stop with no `elements`: the tour is dropped |
| `13-unknown-cell-kind` | A cell of an unknown kind: the table is dropped |
| `14-unknown-rank` | A row with a rank that is not in `Rank`: the table is dropped |

Folders 01 to 07 are valid. Folders 08 to 14 are invalid on purpose.

`tests/it/conformance.rs` has one test per case, which checks what Perch's reader does with the
case, by its `expect.md`.

To copy the fixtures again, from the Changescape repo's `contract-fixtures/` at the commit that
`git -C /Users/dmitriy/workspace/changescape rev-parse --short page-v4` prints:

```sh
for d in /Users/dmitriy/workspace/changescape/contract-fixtures/*/; do
  n=$(basename "$d"); mkdir -p "$n"; cp "$d/page.json" "$d/status.json" "$d/expect.md" "$n/"
done
```

Then update the commit above, and the tests that name a case.
