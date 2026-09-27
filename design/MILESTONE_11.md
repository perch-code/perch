# Perch: Milestone 11, The diff folds and the threads sit in it

## Goal

Make Files read like a diff. Today a changed file is shown whole, every
unchanged line of it, with the changes tinted somewhere in the middle
of twelve thousand lines. The threads about those lines are in a column
to the right, and a marker in the gutter is the only tie between a
thread and the code it is about.

After this milestone a changed file opens folded: the changed lines,
three lines of context around each, and a divider where the unchanged
run between two hunks is hidden, the way git and GitHub show a diff.
Clicking the divider reveals more, and what was revealed is remembered
for that worktree and that version of the file. A worktree can be told
to show its files whole, and remembers that too.

And every thread, the reader's own with an agent and GitHub's review
comments alike, is drawn in the code under the lines it is about, in
the stacked layout and the split one. The lines a thread is anchored on
are never folded: a thread is a change to how the file reads, and it
keeps its lines in view the way a hunk does.

The design decision the milestone turns on is that both features are
one transform. The editor draws a document; the document is built from
the display buffer by hiding folded rows and reserving rows under a
thread. The gutter, the split's two sides, the jumps and the anchors all
speak the document's rows. Nothing else in the code pane learns about
folds or cards. That is what keeps the editor a single `TextEditor` and
the gutter one row per line.

## Non-goals

- Writing to GitHub. A GitHub comment drawn inline is the same
  read-only block Conversation shows, with the same "ask" that opens a
  Perch thread. No reply box, no resolve.
- Folding files that are not in the changed list. There is nothing to
  fold around. They open whole, as today.
- Folding by syntax. Functions and blocks do not collapse. Only
  unchanged runs of a diff.
- Folding the removed lines. A ghost run is part of a hunk and is
  always shown.
- Remembering reveals across a push. A reveal is keyed to the blob the
  file had. A new blob starts folded again, as Viewed does.
- Remembering the scroll position.
- Cards for threads with no anchor. A worktree thread is on
  Conversation and in the column, not in any file.
- Retiring the thread column. It stays, as the list of every thread in
  the worktree with the ones elsewhere. This milestone changes what is
  drawn in the code and not what the column is. Whether the column is
  still wanted once the cards are inline is a question for after it
  has been used that way for a while.

## Behaviour

### The document

The code pane draws a document, not the display buffer. A document is
a text and a table with one entry per line of the text. An entry is one
of:

- **Line.** A display line of the buffer, as today: a head line or a
  ghost.
- **Filler.** Split only. The other side has a line here and this one
  does not, as today.
- **Divider.** A run of unchanged display lines hidden behind it, with
  how many. One row tall.
- **Spacer.** One of the rows reserved under a line for a thread's
  card. A card `k` rows tall reserves `k` spacers, all naming the
  card.

The stacked layout builds one document from the unified buffer. The
split layout builds two, one per side, from the alignment, and a
divider or a spacer appears in both at the same row so the two stay
the same height. `Rows` in `editor.rs`, which already stands for "the
unified buffer or the head side", becomes the document. `SideDoc`,
which already has this shape, is the document with two more entry
kinds.

### Folds

A changed file, open under any scope, is folded by default: every run
of unchanged display lines longer than six is cut down to three lines
each side of what surrounds it, and the rest is a divider. A run
shorter than that is shown whole; a divider hiding two lines is worse
than the two lines. The run at the top of the file keeps three lines
before its first change and the run at the bottom three after its last,
and either can be a divider from line one or to the end.

What a run is folded around:

- A changed line: added, modified, or a ghost.
- A line a thread is anchored on, the reader's own or GitHub's, from
  the anchor's first line to its last. An outdated thread's anchor
  still points at head lines and counts. A GitHub comment on the base
  side or without a line has nowhere in the head to be, as today, and
  does not count.

Both get the same three lines of context. The folds are a pure function
of the diff and the anchors on the file, computed each time the document
is built, and are never stored.

**The divider** is one row of the document, drawn full width across the
code as a bar in the divider colour, with `47 unchanged lines` in the
middle in the code font. Where a thread's anchor sits in the run the
divider is already broken there, so a divider never has a card in it.

The gutter row beside a divider holds its controls, the way GitHub's
gutter does:

- **Up.** Reveal the next twenty lines at the top of the run.
- **Down.** Reveal the next twenty lines at the bottom of the run.
- **All.** Reveal the whole run. Shown instead of Up and Down when the
  run is forty lines or fewer, since two clicks would do it anyway.
- **Reset.** Fold the run back to its default. Shown only when the run
  has a reveal in it. Hovering the divider shows the controls;
  otherwise the bar shows the count alone.

**A reveal** is a range of head lines the reader asked to see. Up,
Down and All each add one. The document is built from the default folds
less the reveals: a fold that a reveal overlaps is cut back to what the
reveal leaves, and a fold a reveal covers entirely is gone with its
divider. Reveals are in head lines, not display lines, so the stacked
and split layouts read the same set and a reveal made in one holds in
the other. Reset removes every reveal that overlaps the divider's run.

A jump into a fold is a reveal. Go to definition, Find references, a
thread's anchor label in the column, and `]` and `[` all scroll to a
head line, and when that line is inside a fold the fold is revealed
from three lines above the target to three below, and the reveal is
kept like any other. Reset undoes it. `]` and `[` go between changes,
which are never folded, so they reveal nothing in practice; the rule
is there so the jump code has one path.

**Reveals are remembered** per worktree, per path, per head blob. Quit
and relaunch, or close and reopen the item, and the file opens with
the same runs open. A file whose blob changed, because the branch
moved, opens at its defaults; the old blob's reveals stay stored and
apply again if the file comes back to it, which is how Viewed behaves
and for the same reason. Changing scope changes the diff and so the
default folds, and the reveals, being in head lines, apply to whatever
the diff is.

**Show whole** is a toggle on the tab strip beside Stacked and Split,
a tile like theirs, on by default in every worktree, drawn as the
folded-diff glyph. Off, every file in the worktree is shown whole and
no divider is drawn. On again, the folds return and the reveals with
them; turning it off does not throw them away. The setting is per
worktree and remembered in `meta`. The layout toggle is per app and
stays that way; a reader folds one review and not another, but reads
every review stacked or every one split.

### Cards

Every thread anchored in the open file is drawn in the code, directly
under the last line of its anchor: the reader's own threads, and
GitHub's review comment threads that Files today lists under On
GitHub. A thread anchored on lines 41 to 58 has its card under 58,
after the ghost lines if the last line has any spliced under it.

A card takes the rows its height needs, rounded up to whole lines, and
those rows are spacers in the document: the editor draws them blank,
and the card is drawn over them, full width, in a layer above the
editor that scrolls with it. In a split the spacers are in both sides
and the card spans both, as it does on GitHub.

The reader's own thread is the thread card the column draws: the
anchor line, the messages, the run under an agent's message with its
steps collapsed to a line, the follow-up composer collapsed to one
line. A GitHub thread is the nested block Conversation draws for a
line comment: the comment and its replies, with the byline's ask. The
two faces stay different, as Milestone 8 required. A card has no
`src/x.rs:41–58` label of its own, since it is under those lines; it
has the Outdated pill and the `...` menu where the label was.

A card has a chevron at its left that collapses it to one row: the
first author, the first line of the first message, and how many
messages follow. A card starts collapsed when its thread is outdated,
or is a GitHub thread that GitHub says is resolved. Any other starts
open. Whether a card is open is remembered for the life of the app, not
across launches.

The composer for a new thread, opened with `+` or `a` on lines, is a
card too, under the last selected line, with its spacers. Send turns it
into the thread's card in place. Escape removes it and its rows.

The gutter marker where a thread starts stays, hollow for GitHub and
solid for the reader's own, as today. Clicking it scrolls the card into
view and flashes its border, as clicking it scrolls the column today.
`Cmd-Shift-\` still shows and hides the column. The column's rows are
the same; the On GitHub group stays, since the column is the list and
the cards are the code.

### Heights

A card's height is not known until it is laid out, and changes while an
agent's answer streams in. The document is built with an estimate, the
card reports its laid-out height, and the document corrects.

The estimate is from the card's text: one row per line of every
message body after wrapping at eighty columns, plus two rows per
message for the byline and the gap, plus one for the anchor row and one
for the follow-up line, capped at forty rows. A collapsed card is one
row. The estimate is a starting point and can be wrong by a few rows;
it only has to be near enough that the first paint is not a jump.

The correction is an edit. When a card's laid-out height, rounded up to
rows, differs from the rows it has, the editor's document is edited in
place: blank lines inserted or removed at the card's spacers, and the
document's table updated to match. The editor is not rebuilt. A rebuild
loses the scroll position and the layout cache, and a streaming answer
would rebuild it every line. The document is read-only to the reader,
not to the program.

A card is never taller than forty rows on screen. A thread longer than
that scrolls inside its card. The cap is what keeps a long conversation
from pushing the code off the screen; the column is there for reading
one at length.

### The status line

The status line's `4 changes` is unchanged. Beside it, when the open
file has folds, `312 lines hidden`. Clicking it is Show whole for this
worktree.

## Model

`perch-git`:

- `DisplayBuffer` is unchanged. Folds and cards are the app's.
- `FileDiff` gains nothing. The unchanged runs are read off the
  buffer's rows.

`perch-app::review::doc`, new:

- `DocRow`: `Line(usize)`, `Filler`, `Divider { run: usize }`,
  `Spacer { card: CardId, row: usize }`, where `run` indexes the file's
  runs and `row` is which of the card's spacers this is.
- `Doc { text: String, rows: Vec<DocRow> }`, replacing `SideDoc` and
  the `Rows` enum. `display_line(row)`, `row_of_display(display)`,
  `row_of_head(head)`, `head_position(offset)`,
  `nearest_display_line(row)` as `Rows` has them today, over the new
  rows: a divider and a spacer are no line of anything, like a filler.
- `Run { first: usize, last: usize }`, a run of unchanged display
  lines, and `runs(buffer, anchors) -> Vec<Run>`: the runs longer than
  six with the context taken off each end, given the head line ranges
  that count as anchors. Pure. This is the default fold set.
- `Reveals`, a set of head line ranges, with `add`, `remove_overlapping`,
  and `apply(&[Run]) -> Vec<Run>`: the runs after the reveals are cut
  out of them.
- `CardId`: `Thread(String)` for the reader's own by thread id,
  `GitHub(u64)` for a GitHub thread by its first comment's id,
  `Composer` for the one being written.
- `Card { id, after_display_line: usize, rows: usize }`.
- `build(buffer, folds: &[Run], cards: &[Card]) -> Doc` for the stacked
  layout, and `build_split(alignment, base, head, status, folds, cards)
  -> (Doc, Doc)` for the split, replacing `split_docs`. Both walk the
  rows in order and emit a divider where a run starts, skip the run's
  lines, and emit `rows` spacers after a card's line.
- `estimate_rows(&CommentThread) -> usize` and the same for a
  `LineThread`, the estimate above. Pure, with tests.

`perch-app::review::model`:

- `LoadedFile` gains `blob: String`, the head blob, which the load
  already has for Viewed.
- `ReviewState` gains:
  - `folded: RwSignal<bool>`, the worktree's Show whole, true meaning
    folded. Set from `meta` when the item opens.
  - `reveals: RwSignal<HashMap<(String, String), Reveals>>` by
    `(path, blob)`, loaded with the viewed set when the item opens.
  - `card_rows: RwSignal<HashMap<CardId, usize>>`, the rows each card
    has now, estimate or corrected.
  - `collapsed_cards: RwSignal<HashSet<CardId>>`.
  - `focus_card: RwSignal<Option<CardId>>`, replacing `focus_thread`,
    since a GitHub thread can be focused too.
- `anchors_in(path) -> Vec<(u32, u32)>`, every anchored head line
  range in a file, the reader's threads and GitHub's, for `runs`.
- `reveal(path, blob, first, last)` and `reset(path, blob, run)`, which
  update the signal and tell the backend.
- `jump_to_line` handling in `wire_jump` calls `reveal` around the
  target when the target's row is inside a fold, before scrolling.

`perch-app::review::editor`:

- `stacked_view` and `split_view` build their documents from the
  signals: the buffer, `folded`, the file's reveals, the anchors, and
  `card_rows`. The document is built once per editor and edited after.
- `code_editor` returns an editor whose document handle is kept, so
  the height correction can edit it.
- A card layer: a `dyn_stack` over the cards of the open file, each
  positioned at its first spacer's row times the line height less the
  viewport's top, in a container that sits over the editor and takes
  pointer events only where a card is. The gutter already positions
  its rows this way from the same `viewport` signal.
- A divider layer, the same way, for the bars.
- `on_resize` on each card writes `card_rows` when the rounded height
  differs, and an effect over `card_rows` applies the edit.

`perch-app::review::gutter`:

- `gutter_row` and `side_row` take a `DocRow`. A divider row draws the
  controls; a spacer draws nothing and records no hover; a line is as
  today.

`perch-app::review::threads`:

- `ThreadFace::Inline`, a third face: the column's card without its
  anchor label, with the chevron.
- `thread_card` takes the face. `line_thread_block` in `cards.rs`
  likewise, for the GitHub thread's inline form.

`perch-app::review::tabs`: the Show whole tile beside the layout tiles.

## Storage

`perch-github::cache`:

- A `reveals` table: `item_id`, `path`, `blob`, `first`, `last`, primary
  key over all five, with `set_reveal`, `remove_reveals(item_id, path,
  blob, first, last)` for every row overlapping the range, and
  `load_reveals(item_id) -> HashMap<(path, blob), Vec<(first, last)>>`.
  Keyed as `viewed` is, for the same reason.
- `meta` key `review.folded.<item_id>`, `1` or `0`, absent meaning
  folded. Added to the list of keys the launch reads. No key is
  removed and none renamed.
- Schema version to 13. Recreate, as every change so far has. What is
  lost is the same as Milestone 10 said, and the commit message says
  so again.

## Backend

- `Command::Reveal { item_id, path, blob, first, last }` and
  `Command::ResetReveals { item_id, path, blob, first, last }`, written
  through as `SetViewed` is.
- `Command::SetFolded { item_id, folded }`, a `meta` write.
- `Event::ViewedReady` carries the reveals too, loaded in the same
  query pass when the item opens, or a sibling `RevealsReady` if the
  viewed load is better left alone.
- The file load records the head blob on `LoadedFile`.

## Harness

`perch --dump-file owner/repo#N path` renders the document, not the
buffer: dividers as `··· 47 unchanged lines ···` on their own row,
spacers as `[thread abc123, row 2 of 5]`, gutter numbers and bars as
today. `--whole` renders the buffer as the flag does today. `--split`
renders both sides, one after the other, each with its dividers and
spacers, so the same row count on each can be checked by eye and by
test.

## Build order

1. `review::doc`: `DocRow`, `Doc`, `Run`, `runs`, `Reveals`, `build`,
   `build_split`. Move what `Rows` and `SideDoc` do into `Doc`, and
   `split_docs` into `build_split`, with no folds and no cards yet, so
   the pane draws what it draws today over the new type. Tests: the
   row mappings on a document with fillers match what `Rows` gave;
   `runs` on a buffer with two hunks forty lines apart is one run with
   three lines off each end; a run of six is no run; a run that an
   anchor sits in is two runs, each with context on the anchor's side;
   a reveal cuts a run, covers a run, and straddles two. Commit.
2. Folds in the stacked layout, no persistence: `folded` defaults
   true, `build` takes `runs` less an in-memory `Reveals`, the divider
   row is drawn and its gutter controls work, `wire_jump` reveals
   around a target. `--dump-file` prints dividers. Commit.
3. Folds in the split: `build_split` emits the divider on both sides at
   the same row, `side_row` draws the controls on the right. Test: both
   documents have the same row count for a file with three runs and
   two reveals. Commit.
4. Persistence: the table, the commands, the load, the `meta` key, the
   Show whole tile and the status line count. Schema to 13. Commit,
   saying what is lost.
5. Cards, the reader's own threads: `CardId`, `Card`, `estimate_rows`,
   spacers in `build` and `build_split`, the card layer,
   `ThreadFace::Inline`, the chevron and `collapsed_cards`, the marker
   click into `focus_card`. Heights are the estimate only. Commit.
6. The height correction: the document handle, `on_resize`, the edit in
   place. Test in the headless harness: a card whose rows change from
   five to eight leaves the lines below it three rows further down and
   the scroll where it was. Commit.
7. GitHub threads as cards, and the composer as a card. `runs` takes
   the GitHub anchors from step 1's signature already; here they are
   drawn. Commit.
8. README: Files gets a paragraph on folding and one on the cards; the
   keys table is unchanged; the harness section gets `--whole` and
   `--split`. Commit.

Step 1 changes no behaviour and can land alone. Steps 2 to 4 are folds
and are usable without a card. Steps 5 to 7 are cards and need step 1
and nothing after it, so they can be built beside the folds if two
people are on it.

## Acceptance

- Open a pull request with a file of ten thousand lines and four
  hunks. The file is a few screens: each hunk with three lines around
  it, a divider between, a divider from line one and one to the end.
  The status line says how many lines are hidden. Split; the same
  dividers on both sides, the same height.
- Hover a divider. Up, Down. Click Down; twenty more lines and the
  count drops by twenty. Click Down until the count is under forty; the
  controls are All. Click All; the divider is gone. Reset is on the
  divider above it. Click it; the run folds to its default.
- Quit and relaunch. The runs are open where they were left. Push a
  commit that touches the file; it opens at its defaults. Check out the
  old head; the reveals are back.
- Click Show whole. Every file whole, no divider, the status line count
  gone. Open another item; it is folded, because the setting is per
  worktree. Back to the first; whole. Click again; folded, with the
  reveals as they were.
- Ask about lines 2160 to 2162 of a file where nothing else has changed
  for a thousand lines around them. The three lines and three each side
  are in view, with dividers above and below, and the card is under
  2162. Delete the thread; the run closes over the lines.
- Shift-F12 on a symbol, pick a reference inside a fold. The file
  scrolls there with the fold open around it, and the divider above it
  offers Reset.
- The card under a thread has the messages, the run, the follow-up.
  Type a follow-up and send. The answer streams in and the card grows;
  the code below it moves down and the code above it does not move.
  The scroll position holds. Nothing flickers.
- Click the chevron. One row, with the author and the first line.
  Click it; the card again. The lines below it were where they should
  be both times.
- A GitHub review comment on line 2202 with two replies is a card under
  2202, in the GitHub face, with ask in its byline. The gutter marker is
  hollow. It starts collapsed if GitHub says resolved.
- Split. Cards span both sides. Stacked; the same cards.
- `perch --dump-file` on the file prints the dividers and the spacers;
  with `--whole`, neither; with `--split`, two renders of the same row
  count.
- `cargo test` green, `cargo clippy` clean.

## Things the implementer should not do

- Do not put folds or cards into `DisplayBuffer`. The buffer is the
  file with the diff spliced in and belongs to `perch-git`. Folds and
  cards are the app's document over it.
- Do not keep `Rows`, `SideDoc` and `Doc` side by side. `Doc` is the
  one type, and the stacked layout uses it too.
- Do not store folds. Store reveals. A fold is derived from the diff
  and the anchors every time; a stored fold would be wrong after the
  next push and would have to be reconciled.
- Do not keep a second, unstored reveal set for jumps. A jump reveals
  like a click does, and Reset undoes it like a click's.
- Do not fold around a GitHub comment on the base side, or one with no
  line. It has no head line, and today it is not drawn in Files at all.
- Do not rebuild the editor when a card's height changes. Edit the
  document. If floem's read-only editor refuses a programmatic edit,
  that is the first thing to fix, in the fork, and not a reason to
  rebuild.
- Do not draw the card as phantom text. Phantom text is text; a card
  has a composer and a menu.
- Do not let a card be taller than forty rows. Scroll inside it.
- Do not draw the divider's controls in the editor's text. They are the
  gutter's, and the bar over the editor is a label.
- Do not make Show whole per app or per file. Per worktree. The layout
  toggle stays per app; the two are different kinds of preference and
  the README should say why in a line.
- Do not remove the thread column, the On GitHub group, or
  `Cmd-Shift-\`. That is a later decision.
- Do not change how `viewed` is keyed, and key `reveals` the same way.
- Do not write a migration. Bump and recreate, and say what is lost.
