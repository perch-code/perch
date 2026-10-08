# A row whose group is not in groups

Valid. The schema cannot check that a row's `group` names a group in `groups`.

The row `sub()` names the group "calc.gone", which `groups` does not list, so it belongs to no
group. The table draws the rows without a group first: `sub()`. Then one section with the header
"calc.ops" and the summary "2 functions", open, with `add()` and then `mul()`.
