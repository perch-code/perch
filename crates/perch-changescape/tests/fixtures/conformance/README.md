# Conformance fixtures

Perch keeps only the cases it reads: a page without a tour, a tour, and a tour that does not
read. The Changescape repo's other cases test page versions and view blocks, which Perch does
not read.

Each folder holds a `page.json`, a `status.json`, and an `expect.md` that says what a correct
reader shows. The contract is `docs/page-data.md`. The shapes are in `../page.v3.schema.json`
and `../status.schema.json`.

| Fixture | Case | Matches the schemas |
| --- | --- | --- |
| `01-minimal` | A minimal valid page, with no tour | yes |
| `12-tour` | A tour of one chapter and one stop, with a call diagram on the stop and as the spine, and a picture kind the reader does not know | yes |
| `13-malformed-tour` | A malformed tour: drop the tour, draw the rest | status yes, page no |

`13-malformed-tour/page.json` is invalid on purpose. It has a stop without `els`, and a picture
without `kind` whose window lacks `base`. Types generated from the schema are strict, so Perch
reads the tour on its own, and then the rest of the page without the views' blocks
(`src/page.rs`).

In the Changescape repo, check the fixtures, or any build, from `skill/schema/`:

```sh
uv run check.py                     # every fixture: valid ones pass, invalid ones fail
uv run check.py <build>/page.json   # one file, by its own format version
```
