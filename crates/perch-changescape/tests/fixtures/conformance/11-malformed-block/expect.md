# A malformed block

Not valid: the first block of Placement is a table without `columns`, and its `rows` is a string.
That is the point of the case.

The reader drops only that block (or shows a short error in its place). It draws the table
"New functions" after it, and the rest of the page, as usual.
