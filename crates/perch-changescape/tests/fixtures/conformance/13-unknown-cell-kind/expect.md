# 13-unknown-cell-kind

Invalid on purpose. A cell has the kind `chart`, which format 4 does not list. `changescape check` must fail this folder.

The only problem is views.0.blocks.0.table.rows.0.cells.number: the tag "chart" is unknown.

A correct reader drops only the table block that holds the cell, or it draws that one cell as text. It must not fail the page. It keeps the other blocks and views.
