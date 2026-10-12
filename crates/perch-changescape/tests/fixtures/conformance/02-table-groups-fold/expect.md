# 02-table-groups-fold

Valid format 4 build. A table with key, normal, and minor rows, two groups, one closed group, and a fold.

A correct reader shows the table "Changed functions" in the Overview view with four rows.

- The row "handle" is a key row. It comes first. Its `why` text is a tooltip, and the reader can show it.
- The rows of the group "Public API" are open. The reader shows "handle_cached" in that group.
- The group "Store internals" is closed. The reader shows its heading and summary "2 functions", and it folds its rows.
- The minor row "touch" is dimmed and folds behind the control.
- The fold shows 2 rows that are not minor. The rest go behind one control labelled "small helpers and the store internals".
- Clicking the row "handle" opens the function `pkg.api.handle`.
