# Conformance fixtures

Small hand-written builds for readers of page format 3. Each folder holds a `page.json`, a
`status.json`, and an `expect.md` that says what a correct reader shows. The contract is
`docs/page-data.md`. The shapes are in `../page.v3.schema.json` and `../status.schema.json`.

| Fixture | Case | Matches the schemas |
| --- | --- | --- |
| `01-minimal` | A minimal valid version 3 page | yes |
| `02-status-without-page-schema` | status.json without `page_schema`: version 2, refuse and offer Rebuild | status yes, page no |
| `03-page-schema-4` | `page_schema` 4: too new, offer the browser page | status yes, page no |
| `04-unknown-rank` | A `rank` the reader does not know: a normal row | yes |
| `05-group-not-in-groups` | A row whose `group` is not in `groups`: no group | yes |
| `06-groups-closed` | A group with `open: false` | yes |
| `07-mixed-fold` | `fold.show` hides normal rows and minor rows behind one control | yes |
| `08-ranks-without-groups` | Ranks with no `groups` and no `fold` | yes |
| `09-facet-rows-without-el` | A facet, and table rows without `el` | yes |
| `10-unknown-block-type` | A block of an unknown `type`: "in the browser page" | status yes, page no |
| `11-malformed-block` | A malformed table block: drop only that block | status yes, page no |

The four page.json files that do not match are invalid on purpose:

- `02-status-without-page-schema/page.json` is a version 2 page (`"schema": 2`).
- `03-page-schema-4/page.json` is a version 4 page (`"schema": 4`).
- `10-unknown-block-type/page.json` has a block of type `heatmap`, which version 3 does not list.
- `11-malformed-block/page.json` has a table block without `columns`, whose `rows` is a string.

A reader must handle all four without failing. Types generated from the schema are strict.
So deserialize each block on its own: first as a JSON value, then as a `Block`. Also check
`schema` and `page_schema` before you parse the rest of the page.

Check the fixtures, or any build, from `skill/schema/`:

```sh
uv run check.py                     # every fixture: valid ones pass, invalid ones fail
uv run check.py <build>/page.json   # one file, by its own format version
```
