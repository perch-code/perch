# Conformance fixtures

Small hand-made builds for readers of page format 4. Each folder holds a `page.json`, a `status.json`,
and an `expect.md`. The `expect.md` says what a correct reader shows, or how it must refuse.
The contract is `skill/src/changescape/contract/page-data.md`. The shapes are in
`skill/src/changescape/contract/page.v4.schema.json` and `status.v4.schema.json`.

| Fixture | Case | Matches format 4 |
| --- | --- | --- |
| `01-minimal` | A minimal valid page: one view with one text block | yes |
| `02-table-groups-fold` | A table with key, normal, and minor rows, two groups, one closed group, and a fold, and a type map block | yes |
| `03-table-ranks` | A table with ranks, no groups, and no fold | yes |
| `04-facet-rows-without-element` | A facet with options, and table rows with no `element` | yes |
| `05-cell-kinds` | A table row with each cell kind: text, number, status, element, file, facet, and list | yes |
| `06-comments` | Each comment kind: inline with a line range, inline with `lines` null, review with a state, and issue; and a discussion block | yes |
| `07-tour` | A tour of one chapter with a type map spine, one stop, a hunk picture and a seq picture, three flags with each target kind, and a below band | yes |
| `08-status-page-schema-3` | `page_schema` 3 in status.json: refuse, and offer a rebuild | status no, page yes |
| `09-page-schema-3` | `schema` 3 in page.json: refuse, and offer a rebuild | page no, status yes |
| `10-unknown-block-type` | A block of an unknown `type` in a view: draw the rest of the view | no |
| `11-malformed-table` | A table block with no `columns`: drop only that block | no |
| `12-malformed-tour` | A stop with no `elements`: drop the tour, and draw the sections | no |
| `13-unknown-cell-kind` | A cell of an unknown `cell` kind: drop that block or cell | no |
| `14-unknown-rank` | A row whose `rank` is not in `Rank`: do not guess a rank | no |

Folders 01 to 07 are valid. Folders 08 to 14 are invalid on purpose. A reader must handle each invalid
folder without failing. `changescape check` must fail each of them.

Advice for a reader that generates types from the schema:

- Check `schema` and `page_schema` before you parse the rest of the page. If either is not 4, refuse.
- Deserialize each block on its own. A block that does not parse drops only that block.
- The generated types are strict. A reader that parses the whole page at once drops the whole page.

Check every fixture, or one build:

```sh
uv run --project skill changescape check                      # every fixture: valid ones pass, invalid ones fail
uv run --project skill changescape check <build>/page.json    # one file, checked against format 4
```

The list of invalid fixtures is `INVALID` in `skill/src/changescape/commands/check.py`.
