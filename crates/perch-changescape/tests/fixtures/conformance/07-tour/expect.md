# 07-tour

Valid format 4 build. A tour of one chapter with a type map as its spine, one stop, a hunk picture and a seq picture on that stop, three flags, and one band.

A correct reader makes the tour the main view. It keeps the sections as the reference.

- The chapter "The change" has one stop: "Store keys must be strings". The stop shows its "before" and "now" text and its watch note.
- The chapter shows its spine, a type map titled "Records of the store", above the stop. It has six boxes: the record `Entry`, the records `Base`, `Legacy`, and `Owner`, the union `Change`, and the interface `Policy`.
  - `Base` is moved, and its `was` is `pkg.old.Base`. It has no rows.
  - `Entry` is changed, and its kind change reads "TypedDict → dataclass". Its row `key` is retyped from `Any` to `str`. Its row `owner` is added. Its row `legacy` is removed. Its folded row counts two same rows.
  - `Legacy` is removed, and `Owner` is added.
  - The interface `Policy` is added. It has no rows, and its box says "interface" above its name.
  - The union `Change` has no rows.
  - The arrows are: `Change` to `Entry` (member, tag `kind=entry`, same in both revisions), `Change` to `Legacy` (member, tag `kind=legacy`, removed), `Entry` to `Base` (extends, same), `Entry.legacy` to `Legacy` (holds, removed), and `Entry.owner` to `Owner` (holds, added), and `Owner` to `Policy` (implements, added).
  - The reader draws the arrows with their marks. It lays out the boxes itself. The map has a note that explains the marks.
- The stop shows the seq picture first. A click on the node `n1` opens the element `pkg.store.Store.put`.
- The stop shows the hunk picture of `pkg/store.py`, with one window: base lines 14 to 15 and head lines 14 to 17.
- The reader shows three flags before the first chapter.
  - "Check the key type" opens the element `pkg.store.Store.put`.
  - "Read the API" opens the view Overview.
  - "A reviewer asked a question" opens the inline comment 101.
- The band "Tests" is below the chapter. It shows the file `tests/test_store.py` with 5 added lines. The reader skims it.
- The ending shows one check and one action, "Run the tests".
