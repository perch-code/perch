# 04-facet-rows-without-element

Valid format 4 build. A facet with two options, and table rows that have no `element`.

A correct reader shows the table "Changed functions" with two rows: "handle" and "touch".

- Neither row opens an element, because neither row names one.
- The reader shows the facet "Part" in the page header, with the options "New code" and "Existing code".
- Applying "New code" keeps the element `pkg.api.handle` in full view. The other rows are dimmed. The row "touch" has no element, so it stays as it is.
- Applying "Existing code" has no element to keep, so the reader dims everything in the table.
