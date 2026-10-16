# 12-malformed-tour

Invalid on purpose. The stop `put-checks-key` has no `elements`. `changescape check` must fail this folder.

The only problem is tour.chapters.0.stops.0.elements: "Field required".

A correct reader drops the whole tour, because the stop has no list of elements. It draws the sections and the views as before, with the Overview text and no tour view.
