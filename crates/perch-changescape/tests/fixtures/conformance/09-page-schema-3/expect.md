# 09-page-schema-3

Invalid on purpose. The page has `schema` 3. `changescape check` must fail this folder.

The status file is valid format 4. Only page.json fails, with the problem "schema: Input should be 4".

A correct reader checks `schema` before it parses the rest of the page. It refuses the page and says that the page is version 3. It offers a rebuild. It draws nothing from the page.
