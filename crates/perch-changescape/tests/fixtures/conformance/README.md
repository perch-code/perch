# Conformance fixtures

These are the cases that Perch reads, in page format 4: a page without a tour, and a tour.

Each folder holds a `page.json`, a `status.json`, and an `expect.md` that says what a correct
reader shows. The shapes are in `../../../schema/page.v4.schema.json` and
`../../../schema/status.v4.schema.json`. The meanings are in `page-data.md` in the Changescape
repo, under `skill/src/changescape/contract/`.

| Fixture | Case |
| --- | --- |
| `01-minimal` | A minimal page, with no tour |
| `12-tour` | A tour of one chapter with a read stop and a skim stop, each with a hunk picture |

The `12-tour` hunk header said 10 added lines over 5 rows in format 3. The format 4 span keeps
that count, so the span and the rows do not agree. No test depends on the count.

Both files of each case are valid against the format 4 schemas. To check a file, run the
`jsonschema` package from the Changescape skill's environment:

```sh
~/.claude/skills/changescape/.venv/bin/python -I -c "import json,sys,jsonschema; \
  [print(e.json_path, e.message) for e in jsonschema.Draft202012Validator( \
  json.load(open(sys.argv[1]))).iter_errors(json.load(open(sys.argv[2])))]" \
  ../../../schema/page.v4.schema.json 12-tour/page.json
```
