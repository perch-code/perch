# 08-status-page-schema-3

Invalid on purpose. The status file has `page_schema` 3. `changescape check` must fail this folder.

The page.json is valid format 4. Only status.json fails, with the problem "page_schema: Input should be 4".

A correct reader refuses to draw this build. It says that the build is version 3 and that this reader needs version 4. It offers a rebuild. It does not parse the page.
