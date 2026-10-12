# 07-tour

Valid format 4 build. A tour of one chapter with one stop, a hunk picture and a seq picture on that stop, three flags, and one band.

A correct reader makes the tour the main view. It keeps the sections as the reference.

- The chapter "The change" has one stop: "Store keys must be strings". The stop shows its "before" and "now" text and its watch note.
- The stop shows the seq picture first. A click on the node `n1` opens the element `pkg.store.Store.put`.
- The stop shows the hunk picture of `pkg/store.py`, with one window: base lines 14 to 15 and head lines 14 to 17.
- The reader shows three flags before the first chapter.
  - "Check the key type" opens the element `pkg.store.Store.put`.
  - "Read the API" opens the view Overview.
  - "A reviewer asked a question" opens the inline comment 101.
- The band "Tests" is below the chapter. It shows the file `tests/test_store.py` with 5 added lines. The reader skims it.
- The ending shows one check and one action, "Run the tests".
