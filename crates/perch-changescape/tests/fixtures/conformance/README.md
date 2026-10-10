# Conformance fixtures

Perch keeps only the cases it reads: the page versions and the tour. It does not read the views'
blocks, so the Changescape repo's cases 04 to 11, about tables and blocks, are not copied here.

Small hand-written builds for readers of page format 3. Each folder holds a `page.json`, a
`status.json`, and an `expect.md` that says what a correct reader shows. The contract is
`docs/page-data.md`. The shapes are in `../page.v3.schema.json` and `../status.schema.json`.

| Fixture | Case | Matches the schemas |
| --- | --- | --- |
| `01-minimal` | A minimal valid version 3 page | yes |
| `02-status-without-page-schema` | status.json without `page_schema`: version 2, refuse and offer Rebuild | status yes, page no |
| `03-page-schema-4` | `page_schema` 4: too new, offer the browser page | status yes, page no |
| `12-tour` | A tour of one chapter and one stop, with a call diagram on the stop and as the spine, and a picture kind the reader does not know | yes |
| `13-malformed-tour` | A malformed tour: drop the tour, draw the sections | status yes, page no |

The three page.json files that do not match are invalid on purpose:

- `02-status-without-page-schema/page.json` is a version 2 page (`"schema": 2`).
- `03-page-schema-4/page.json` is a version 4 page (`"schema": 4`).
- `13-malformed-tour/page.json` has a stop without `els`, and a picture without `kind` whose
  window lacks `base`.

A reader must handle all three without failing. Types generated from the schema are strict.
So Perch checks `schema` and `page_schema` first, then reads the tour on its own, and then the
rest of the page without the views' blocks (`src/page.rs`).

In the Changescape repo, check the fixtures, or any build, from `skill/schema/`:

```sh
uv run check.py                     # every fixture: valid ones pass, invalid ones fail
uv run check.py <build>/page.json   # one file, by its own format version
```
