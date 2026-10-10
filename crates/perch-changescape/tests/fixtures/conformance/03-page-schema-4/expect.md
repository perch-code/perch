# A page that is too new

status.json is valid against status.schema.json, with `"page_schema": 4`. page.json says
`"schema": 4`, so it does not match page.v3.schema.json. That is the point of the case.

A reader whose highest version is 3 does not draw the page. It says that the page needs a
newer reader, and offers the browser page (page.html) when the build folder has one.
