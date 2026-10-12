# 11-malformed-table

Invalid on purpose. The table block has no `columns`. `changescape check` must fail this folder.

The only problem is views.0.blocks.0.table.columns: "Field required".

A correct reader drops only this table block. It reads the block on its own before it parses the page. The text of the view and the other blocks still show. The reader says that it could not read the table.
