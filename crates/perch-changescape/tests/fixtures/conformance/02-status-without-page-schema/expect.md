# A status without page_schema

status.json is valid against status.schema.json. It has no `page_schema`, so the page counts as
version 2. page.json says `"schema": 2`, so it does not match page.v3.schema.json. That is the
point of the case.

A reader that draws versions 3 and up does not draw the page, and need not parse page.json. It
says that an older Changescape built the page, and offers Rebuild.
