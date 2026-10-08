# An unknown rank value

Valid. `rank` is an open string in the schema.

The Placement table draws three rows in array order: `add()`, `sub()`, `mul()`. `add()` is a key
row (marked as one, with the tooltip "used from 3 packages"). `sub()` has rank "pinned", which
the reader does not know, so it is a normal row: no mark, not folded. The reader can still show
its `why` as a tooltip. Nothing folds.
