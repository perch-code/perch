# A minimal valid version 4 page

Valid against page.v4.schema.json and status.v4.schema.json.
The schema version must be 4. The page has no tour, so `tour` is null.

A reader draws the page. The nav shows four sections: Overview, Map, Behavior, and Code.
Behavior has no views. It shows as one quiet line with its `reason`.

Overview shows the narration, which names `add()` and `calc.ops`. Placement shows a table with
one row, `add()`, status added. A click on the row opens the element panel for `add()` with its
two added lines. Code shows `calc/ops.py` with one hunk of two added lines.
