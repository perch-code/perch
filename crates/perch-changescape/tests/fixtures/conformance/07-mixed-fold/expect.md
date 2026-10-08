# A fold that holds normal and minor rows

Valid.

The table has no groups. It has four normal rows and two minor rows, and `fold.show` is 2. The
reader shows `add()` and `sub()`. One control holds the other four rows: `mul()`, `div()`,
`clamp()`, and `sign()`. Not all of these rows are minor. So the control gives the count, and
the count of minor rows with the label, such as "Show 4 more (2 minor: small helpers)". A click shows
the four rows in array order.
