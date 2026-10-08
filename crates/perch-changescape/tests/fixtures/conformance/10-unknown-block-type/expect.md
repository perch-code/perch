# A block of an unknown type

Not valid: page.v3.schema.json lists the block types of version 3, and "heatmap" is not one of
them. That is the point of the case. A future producer can add a block type without a version
bump. A reader must not fail on it.

The reader draws the page. In Placement, in place of the first block, it says that this block
("Churn by module") is in the browser page, and offers page.html. It draws the table after it as
usual.
