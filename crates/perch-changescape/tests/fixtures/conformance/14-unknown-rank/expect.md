# 14-unknown-rank

Invalid on purpose. The second row of the table has the rank `high`, which is not in `Rank`. `changescape check` must fail this folder.

The only problem is views.0.blocks.0.table.rows.1.rank: "Input should be 'key', 'normal' or 'minor'".

A correct reader must not guess a rank for the row. It drops the table block, or it stops reading that row. It keeps the rest of the page.
