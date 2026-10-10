# A malformed tour

status.json is valid. page.json does not match: its one stop has no `els`, its picture has no
`kind`, and the picture's window has no `base` and a head range of one number.

A reader drops the tour and draws the sections, as for a page without `tour`. It does not fail.
