# Perch: Milestone 14, Code navigation

## Goal

Make Files read like an IDE. Rest the pointer on a symbol and a tooltip
says what it is. Cmd-click it and its definition opens. Right-click it
and a menu offers the rest: type definition, implementations,
references, callers. The keys are VS Code's, so a reader who knows them
does not have to learn Perch's.

Today the language server answers three questions, and the way to ask
them is a click. Clicking a symbol asks rust-analyzer about it and the
answer arrives in a strip under the code, with Go to definition and Find
references as buttons in the strip's header. That was the right first
shape: it needed no pointer coordinates and it never covered the diff.
It is also nothing like what a reader's hands expect. A reader who
Cmd-clicks gets a caret. A reader who rests the pointer gets nothing.
A definition in a dependency, which is half of what a Rust reader asks
about, is dropped on the floor.

After this milestone the strip is gone. Hover is a tooltip, floem's own,
opened by the pointer resting and closed by it moving. Cmd-click and F12
go to the definition, inside the worktree or out of it. The references
panel learns to list implementations and definitions too, and gains a
sibling that shows a call hierarchy as a tree. The caret resting on a
symbol lights its other uses in the file. All of it is read-only, and all
of it is rust-analyzer: the client is generic and the milestone adds
requests to it, not servers.

The design decision the milestone turns on is that the tooltip is a
tooltip. It opens where the pointer is, it cannot be entered, and what
does not fit in it is cut. The panel under the editor is where a list
goes; the tooltip is a glance. VS Code's hover can be scrolled and
clicked into, and the cost is a widget that has to be dismissed. Perch's
goes away when the pointer does.

## Non-goals

- Editing. No completion, signature help, rename, code actions or
  formatting. Files is read-only and stays so.
- Diagnostics. They arrive today and are dropped in the backend. Drawing
  them in a diff needs a way to tell a warning the pull request
  introduced from one that was there, and that is a milestone of its
  own.
- Inlay hints, code lens, semantic tokens, document symbols, an outline,
  breadcrumbs. Tree-sitter highlights well enough, and a folded diff has
  no room for hints in the line.
- Workspace symbols. Perch opens what the pull request touched. It is
  not a project browser.
- Type hierarchy. Rare in Rust review; the call hierarchy panel is
  shaped so it could take one later.
- A peek widget. VS Code embeds an editor in the editor. Perch's bottom
  panel is the peek, and one panel is easier to place than a widget
  that has to fit between two lines.
- The underline under a symbol while Cmd is held. The pointer becomes a
  hand, which says the same thing without a text layer. The underline
  needs a decoration the editor does not draw today.
- The base side of a split. rust-analyzer sees the head. A symbol on a
  removed line is nowhere in the head file and asks nothing, as today.
- Servers other than rust-analyzer. `find_server` is where one would go.
- Tuning. No setting for the delay, the tooltip's width or the panel's
  height.

## Behaviour

### Hover

Rest the pointer on an identifier in the code and, after 500 ms still, a
tooltip opens below and to the right of the pointer with what
rust-analyzer says about it. Move the pointer and the tooltip closes.
Press a key or click and it closes. It does not open over whitespace,
punctuation, a removed line, a divider, a filler row or a card.

The tooltip is floem's `tooltip` view wrapped around the editor, with
its `Delay` set to 500 ms. floem owns the timer, the placement, and the
closing. Perch owns the content.

The content is rust-analyzer's hover, asked for as markdown now rather
than plain text, and rendered with the markdown the conversation cards
already use. The first fenced block is the signature, in the code font;
what follows is the doc comment. The doc comment is cut at twelve lines
with an ellipsis on a line of its own, the way the strip cuts it today.
The tooltip is at most 560 px wide and wraps inside that. It has no
scroll: it cannot be entered, so a scroll bar would be a lie. A reader
who wants the whole doc comment goes to the definition.

The ask does not wait for the tooltip's timer. The pointer moving over
the editor is tracked: its point maps to an offset through the editor's
`offset_of_point`, the offset to a head line and column through
`Doc::head_position`, and the column to the identifier around it through
`references::identifier_at`. When the identifier under the pointer
changes, and 150 ms pass with it still the same, the backend is asked
for hover with a token. The answer is kept beside the identifier it
answers. By the time the tooltip fires the answer is usually there.

The tip closure returns a `dyn_container` over the answer signal, so an
answer that lands after the tooltip opened fills it in place, and a
pointer that rests on nothing, or on a symbol rust-analyzer has nothing
to say about, draws nothing at all: an empty view with no chrome. The
chrome is on the card inside the tip, not on floem's `TooltipClass`, so
that nothing means nothing and not an empty box.

While Cmd is held and the pointer is over an identifier, the pointer is a
hand. The pointer's modifiers arrive with every move, so this is a
signal the editor's cursor style reads.

The strip under the code, its buttons and `hover_text` go. Clicking a
symbol places the caret and asks the server nothing. The cursor effect
still records `last_position`, since the keys read it.

### Go to definition

Cmd-click an identifier and its definition opens. F12 with the editor
focused does the same for the identifier at the caret. Either goes
through the definition command as the strip's button does today.

One location opens. Several open the panel as "Definitions of `x`", the
references panel with a different heading, and the reader picks. None
does nothing visible: the status line already says when the server is
not there, and a symbol without a definition is an ordinary thing to
click on.

A definition inside the worktree opens as it does today: the file's tab,
scrolled to the line, a fold revealed around it if it was folded. A
definition outside the worktree, in a dependency or the standard
library, opens too. See below.

Cmd-click also places the caret, since the editor's own handler runs.
That is fine. The caret is where the reader clicked.

Every jump pushes where it left onto a stack: the file and the head
line. `Ctrl--` goes back, `Ctrl-Shift--` goes forward, VS Code's keys on
a Mac. The stack is per item and lives as long as the item is open. Back
from a dependency tab lands in the worktree file that jumped there. A
jump into a file that is no longer open reopens it. The stack holds fifty
places and drops the oldest.

### Files outside the worktree

A location under `~/.cargo/registry`, `~/.cargo/git`, or the toolchain's
`rust-src` opens as a tab like any other. The tab is titled by the
file's name and carries a chip in the muted colour naming where it is
from: `tokio 1.40`, `std`, `git: floem`. The chip's text is derived
from the path: the crate directory's name and version under the
registry, the checkout's name under git, and `std`, `core` or `alloc`
under `rust-src`. Anywhere else under the home directory reads as its
path relative to the home directory, and anywhere else as the path.

The file opens whole, unfolded, with no diff gutter, no cards, no rail
row and no Viewed. It is highlighted. Hover, Cmd-click, references, the
menu and the keys all work in it: the backend sends `didOpen` for it as
for a worktree file, and rust-analyzer already knows it. Close it as any
tab. It is not remembered across the item being closed and reopened.

A file that is not valid text, or is larger than the loader's limit,
opens as the truncated view a worktree file does.

### The menu

Right-click in the code and a menu opens for the symbol under the
pointer:

| Entry | Key |
|---|---|
| Go to Definition | `F12` |
| Go to Type Definition | |
| Go to Implementations | `Cmd-F12` |
| Find All References | `Shift-F12` |
| Show Call Hierarchy | `Shift-Alt-H` |
| | |
| Copy | `Cmd-C` |

The menu is floem's `context_menu` on the editor. Its closure gets no
position, so it reads the symbol the hover tracking last saw under the
pointer, which at the moment of the click is the one clicked. On
whitespace or a removed line the menu is Copy alone. The entries name
what they act on: "Go to Definition of `parse`".

Each key does what its entry does, with the editor focused, for the
symbol at the caret. The keys are caught the way `Shift-F12` is today,
on the way down, before the editor keeps them.

### Implementations, type definition

Go to Implementations asks `textDocument/implementation`: the impls of a
trait, or of a trait method. One opens; several open the panel as
"Implementations of `Read`", grouped by file, with the same rows the
references list has. Go to Type Definition asks
`textDocument/typeDefinition`: the type of a binding, the struct behind
a `Self`. It nearly always has one answer and opens it; several list.

The panel is the references panel with a kind. Its heading, its summary
("4 in 2 files, and 1 elsewhere") and its rows are the same; the kind
picks the words. Opening one panel replaces the other, and Escape closes
whichever is open, as today.

### Call hierarchy

Show Call Hierarchy on a function opens the panel as "Callers of `run`":
a tree. The root row is the function, with its path and line. Under it,
one row per caller: the caller's name, the file and line of the call,
and the call's line of text with the callee emphasised the way the
references rows emphasise the symbol. Click a row and the file opens at
the call. A chevron on a row expands it, which asks for that caller's
callers; each expansion is one request and the tree goes as deep as the
reader takes it. A row already open above it in the tree, a recursive
call, expands to nothing and says so in the muted colour.

A toggle in the panel's header switches to "Callees of `run`", the same
tree the other way. The direction is remembered while the panel is open
and resets to callers when it is opened again.

A caller in a dependency is a row like any other, and opens the
dependency tab. A function with no callers reads "No callers." The menu
entry on something that is not a function, a type or a local, asks
`prepareCallHierarchy`, gets nothing, and the panel does not open.

The tree is a `perch_lsp::lsp_types::CallHierarchyItem` per row, since
the expansion has to send the item back as the server gave it.

### Document highlight

When the caret rests on an identifier, after 150 ms still, every other
use of it in the file is tinted: a subtle background on the range, the
badge colour at low alpha, reads and writes alike. The caret moving off
the identifier clears them. The ranges come from
`textDocument/documentHighlight`, are head positions, and map to rows
through `row_of_head`; a use inside a fold is not drawn, and a reveal
that brings it into view draws it then, since the document rebuilds
through the same styling.

The tint is a layout style in `PerchStyling`, beside the diff
backgrounds, on the range rather than the line. It is under the
selection, so a selection over a highlighted word still reads as a
selection.

### Status

Unchanged. The status line says what rust-analyzer is doing. While it is
absent or failed, the tooltip is nothing, the menu's navigation entries
do nothing, and the keys do nothing. Nothing says so twice.

## Model

In `review::model`:

- `Symbol { path, line, column, end, text }`: an identifier at a head
  position, the column its start, `end` its end. What the pointer is
  over, and what the caret is on.
- `pointer_symbol: RwSignal<Option<Symbol>>`, kept by the pointer
  tracking. `None` over nothing.
- `hover: RwSignal<Option<HoverAnswer>>`, where `HoverAnswer { symbol,
  markdown, token }`. The tip draws it only when `symbol` is the pointer's.
- `highlights: RwSignal<Arc<Vec<(usize, usize, usize)>>>`: head line,
  start column, end column. Empty is none.
- `hover_text` and `dismiss_hover` go. `last_position` stays.
- `References` gains `kind: Locations` with `References`,
  `Implementations`, `Definitions`. `summary` and the heading read it.
- `Hierarchy { root: CallHierarchyItem, direction: Direction, rows:
  Vec<HierarchyRow> }` and `hierarchy: RwSignal<Option<Arc<Hierarchy>>>`,
  where a row is an item, its depth, its call sites, whether it is
  expanded, whether its children are pending, and whether it repeats an
  ancestor. The tree is flat with depths, the way the sidebar's lists
  are, so `dyn_stack` draws it.
- `Place { path: String, line: usize }` and `history: RwSignal<History>`
  with a back stack, a forward stack, and the fifty-place cap. A jump
  pushes back and clears forward; back pops one and pushes the current
  onto forward.
- `LoadedFile` gains `origin: Origin`, `Worktree` or `Outside { label:
  String }`. `Tab` gains the same, so the strip can draw the chip
  without the file being loaded yet. A tab's path is the absolute path
  for an outside file and the relative one for a worktree file, as
  today; `files` is keyed the same way and the two cannot collide.

## Backend

Commands, each with a token and answered by an event of the same name:

- `Hover` as today, the answer now markdown.
- `Definition`, `TypeDefinition`, `Implementation`: answered with every
  location, as `Located { path: String, outside: Option<String>, line,
  character, text }`. `path` is relative for a worktree file and
  absolute otherwise, `outside` is the chip's label when absolute, and
  `text` is the line, read on the way as references does today. The
  app opens one and lists several; the backend does not decide.
- `DocumentHighlight`: ranges.
- `PrepareCallHierarchy`: the items, or none. `IncomingCalls` and
  `OutgoingCalls`: each takes an item and answers with the calls, each
  call's item and its `from_ranges`, and the text of the first range's
  line.
- `OpenOutsideFile { path }`: reads the file, highlights it, answers
  with a `LoadedFile` whose `origin` is `Outside`, and sends `didOpen`
  for it. The loader's size and text checks apply.

`read_references` becomes `read_locations` and serves every list. The
label for an outside path is a pure function in `perch-app`, tested.

Requests for the panel, references and the hierarchy's calls, wait
twenty seconds rather than five: a large crate still indexing can take
that long to answer them, and the panel says it is pending. Hover,
definition and highlight keep five.

## LSP client

In `perch-lsp`:

- `hover` asks for `MarkupKind::Markdown` first, plain text second.
- `type_definition`, `implementation`, `document_highlight`,
  `prepare_call_hierarchy`, `incoming_calls`, `outgoing_calls`. Each is
  the shape of `definition` and `references`: build the params, request,
  decode, and a null reply is an empty list.
- The client capabilities declare `type_definition`, `implementation`,
  `call_hierarchy` and `document_highlight`, `link_support` false
  throughout, so the answers are `Location`s and not links.
- `Request` gains the new variants, and `request` takes a timeout rather
  than the constant.
- The `hover` example gains modes `type-definition`, `implementation`,
  `highlight`, `callers` and `callees`, each printing one
  `path:line:character` per line, so every request can be checked
  against a checkout from the shell before the UI asks it.

## Harness

`perch --dump-file` is unchanged. The pure parts are tested where they
live: the pointer's point to a symbol through a `Doc` with folds and
ghosts; `identifier_at` at a word's ends; the markdown cut; the outside
label from a registry, git and rust-src path; the history's push, back,
forward and cap; the hierarchy's flattening with a repeated ancestor;
the highlight ranges through `row_of_head` with a fold over one. The
editor's headless tests gain a Cmd-click that asks for a definition and
an F12 that asks the same, and a key test for each new key.

## Build order

1. `perch-lsp`: markdown hover, the six new requests, the capabilities,
   the timeout parameter, the example's modes. Run every mode against a
   slatedb checkout. Commit.
2. The pointer tracking: `pointer_symbol` from the editor's pointer
   moves through `offset_of_point` and `head_position`, the 150 ms
   ask, `hover` in the model. The tooltip over the editor with the
   markdown card. Remove the strip, `hover_text` and `dismiss_hover`.
   The hand while Cmd is held. Commit.
3. Cmd-click and F12 to the definition, the history and its keys.
   Commit.
4. Outside files: `Origin`, `OpenOutsideFile`, the label, the tab chip,
   the whole unfolded editor with no gutter. A definition outside the
   worktree opens. Commit.
5. The panel's kinds: `Locations`, definitions and implementations
   listed, type definition, `Cmd-F12`. The context menu with every
   entry. Commit.
6. Call hierarchy: the panel, the tree, expansion, the direction
   toggle, `Shift-Alt-H`. Commit.
7. Document highlight through `PerchStyling`. Commit.
8. README: the keys and the menu, in a Code navigation section, and a
   line that the tooltip cannot be entered and why. Commit.

## Acceptance

Against a slatedb pull request with rust-analyzer ready:

- Rest the pointer on `StoredAttribute` in a changed file. Half a second
  later a tooltip shows `enum StoredAttribute` in the code font and the
  first lines of its doc comment. Move the pointer one character. It is
  gone. Rest on a `(`. Nothing opens. Rest on a removed line's
  identifier. Nothing opens.
- Rest on a well-documented type. The tooltip ends in an ellipsis and
  is no wider than 560 px. It cannot be scrolled.
- Hold Cmd over `try_from`. The pointer is a hand. Cmd-click. The tab
  shows the impl, scrolled so the line is a few rows down, the fold
  around it revealed. `Ctrl--` returns to where the click was.
- Click on `Result`, press F12. A tab titled `result.rs` opens with a
  `std` chip, whole, no gutter, no Viewed. Hover works in it. Cmd-click
  `Option` in it opens `option.rs` beside it. `Ctrl--` twice is back in
  the pull request's file. Close the item, reopen it: the `std` tabs
  are gone.
- Right-click `Read` in a `impl Read for`. The menu names the entries
  with `Read`. Go to Implementations opens the panel "Implementations
  of `Read`" with every impl grouped by file; one in a dependency reads
  "and 1 elsewhere" and is listed under its chip label. Click a row;
  the file opens at the impl.
- Right-click a space. The menu is Copy.
- Right-click a function, Show Call Hierarchy. "Callers of `flush`"
  lists the callers with the call's line and `flush` emphasised. Expand
  one; its callers appear under it, indented. Expand until a row
  repeats an ancestor; it says so. Toggle to callees. Click a row; the
  file opens at the call. Escape closes the panel.
- Show Call Hierarchy on a struct name. Nothing opens.
- Click into `path` in a function that uses it six times. After a
  moment the other five are tinted. Press an arrow key. The tint
  clears. Do it on a use inside a fold; the folded ones are not drawn;
  reveal the fold; they are.
- Kill rust-analyzer. Rest the pointer: nothing. F12: nothing. The
  status line says failed. Restart the item; everything is back.
- Press `Shift-F12`, `Cmd-F12`, `Shift-Alt-H` with the editor focused and
  the caret on a symbol. Each opens its panel. Press them with a
  composer focused. Each types nothing and opens nothing.

## Things the implementer should not do

- Do not ask the server from the tooltip's timer. The tip closure is
  synchronous and runs once; the ask is on the pointer's motion and the
  tip reads the answer. If floem's tooltip cannot be made to draw
  nothing for an empty tip, fix that in the fork, and do not wrap the
  tooltip in a second delay.
- Do not make the tooltip a panel. No scroll, no buttons, no close
  glyph. The menu has the actions.
- Do not render markdown by hand in the tip. Use `review::markdown`, and
  if it cannot draw a fenced block in the code font, teach it to.
- Do not bring back click-to-hover. A click places the caret.
- Do not put the pointer-to-symbol mapping in `perch-git` or in the
  editor's styling. It is `review::hover`'s, beside `line_col_of_offset`.
- Do not write a second panel for implementations or definitions. One
  panel, one `kind`.
- Do not write the hierarchy as a nested view. Flat rows with depths,
  drawn by `dyn_stack`, like every other list in the app.
- Do not give an outside file a rail row, a Viewed check, a fold, a
  card or a thread. It is a place to read, not part of the review.
- Do not remember outside tabs, the history, or the hierarchy across
  the item closing.
- Do not key `files` by anything new. Absolute and relative paths do
  not collide, and a second map is a second thing to keep in step.
- Do not answer a definition in the backend by opening it. The backend
  reports locations; the app decides whether one opens or several list.
- Do not add a setting for the delay, the width or the timeout.
- Do not block the UI thread on any of it. Every ask is a command; every
  answer is an event with the token that asked.
