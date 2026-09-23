# Perch: Milestone 19, Conversation and Code

## Goal

Put the item's two modes back in the bar, and give agents a place on
the item instead of a drawer beside it.

Since 5c6c11e, Conversation has been a tab at the left of the file
tabs. That made it closable, which Conversation shouldn't be. It
needed a chat tile to bring it back, Cmd-W had to special-case it, and
an issue showed a strip holding one tab. Conversation is the item's
home, not one file among others. After this milestone the bar has two
pills in its middle, **Conversation** and **Code**, as Codex has
Summary and Code. The file tabs, their tiles and the file tree's tile
belong to Code alone.

The Agents inspector leaves the drawer. What it shows splits in two:

- **One item's runs** go on that item's Conversation, as an **Agents**
  row in the summary, beside Checks, that unfolds into a list of the
  item's runs, active and past. A run expands in place to what the
  inspector's detail shows today: request, activity, result and usage.
- **Every item's runs** go in a dropdown from an **Agents** button in
  the bar, next to Submit review. Clicking a run there goes to its
  item, on Conversation, with the Agents row unfolded and that run
  expanded.

The drawer is then only the file tree, and only on Code.

## Non-goals

- Changing how a run reads in its thread. The run block in the agent's
  message (steps, approval cards, work changes, footer) stays as M17
  and M18 left it. The Agents row is an index of the item's runs, not a
  second copy of the thread.
- Folding REVIEW HISTORY into the Agents row. Review groups keep their
  cards at the foot of the conversation. The Agents row lists a
  review's attempts, grouped under the review as the inspector does
  now, so every run of the item is in one list, but the group cards'
  own actions (Open snapshot, Stop all, Review latest) stay where they
  are. Merging the two is a follow-up if the duplication grates.
- App-wide history filters. The inspector's worktree, backend and
  outcome filters go with it. The dropdown shows active runs and the
  most recent finished ones. An item's full history is on that item.
- Counts on the pills. Codex's pills carry none, and the old switch's
  counts were noise.
- A Code view for an item without a checkout. An issue has Conversation
  and no switch, as before 5c6c11e. Start work gives it a checkout, and
  the switch appears.

## Behaviour

### The pills

The item bar is `leading`, the switch, `trailing`, with the switch kept
at the bar's centre as the old `switch.rs` did (leading and trailing
each `flex_grow(1)`, `flex_basis(0)`). The switch is two segments,
**Conversation** and **Code**, labels only, with the active one filled.
It is drawn only while the item has a checkout. Without one, the bar is
the title and the trailing controls.

Each item remembers its mode for the life of the app, as `Pane` does
now. An item opens on Conversation the first time and on the mode it
was left in after. Cmd-Shift-1 picks Conversation and Cmd-Shift-2 picks
Code, from the editor and from a composer, as today.

### Conversation

Conversation is the column as it is now: title, summary, finding
filters, cards, review history, the merge surface, the composer. It has
no tab strip. The notice and a retained snapshot's way back stay in the
summary, where they already are on Conversation.

Conversation can't be closed. Cmd-W on Conversation does nothing, which
is how Codex's Summary behaves. Cmd-]/[ step through Code's file tabs
and don't include Conversation.

### Code

Code is the tab strip and the editor, as Files is now without the
Conversation tab: the file tabs, the snapshot banner and notice, the
change navigation, the file tiles (source, fold, Stacked/Split) and, at
the strip's right end, the **file tree** tile. The chat tile and the
agents tile are gone.

Switching to Code with no file tabs open opens the first changed file,
in tree order. With no changes, the tree opens and the pane reads
`No changes`. Closing the last file tab leaves Code showing that pane
and the tree; it doesn't jump back to Conversation.

A file opened from Conversation (a thread's file link, a finding, work
changes) switches to Code, as it does now.

### The file tree

The drawer holds only the tree, and only on Code. Its tile, Cmd-Shift-E
and the drawer's close button toggle it. Docking against
`MAIN_MIN_WIDTH`, overlaying when there's no room, resizing, and
`drawer.width` all stay. `drawer.view` stores `files` or `closed`, and a
saved `agents` reads as closed, which is what `parse` already does with
anything it doesn't know.

On Conversation the tree is hidden, and Cmd-Shift-E switches to Code
with the tree open. Going back to Code shows the tree if it was open
there.

### The Agents row

A summary row, placed after Checks:

    ✦ Agents   [● 1 running · 4 done ▾]

The pill says what the item's runs are doing, the busiest state first:
`N running`, `N queued`, `N waiting` for a run with a pending approval,
then `N done` and `N failed` for runs that have finished. With nothing
active, it reads `5 runs`, or `Last run failed` in the failure colour
when the latest finished run failed. The row is absent when the item
has never had a run.

Clicking the pill unfolds the list under the summary rows, above the
finding filters. Clicking it again folds the list. Whether it's
unfolded is remembered per item, like the mode. It starts folded, and
opens by itself only when a run is jumped to.

The list is the inspector's list for one item:

- Active runs first, then this item's history, newest first, 20 at a
  time with `Load older runs`.
- A review's attempts are grouped under a row for the review, `Review
  #n · sha · against base`, collapsible as in the inspector.
- Each run row is the inspector's row without its footer's repo and
  worktree, which are always this item: status dot, `Work · Claude ·
  Sonnet · High`, status and elapsed (or queue position), the subject,
  and the latest activity in the code font. Its actions are Cancel,
  Stop or Retry as the run allows, and **Show in thread**, which
  scrolls to the run's thread card and flashes it (`focus_card`).
- Clicking a row expands it in place into the inspector's detail
  sections: REQUEST, ACTIVITY, the result, USAGE. The expanded body is
  capped at 480px high and scrolls on its own. While the run streams it
  follows the bottom, with `Jump to latest` as now. More than one run
  can be expanded. Clicking the row's header again collapses it.

### The Agents button

In the bar's trailing controls, just before the primary action
(Submit review, or Start work on an issue):

    [GitHub] [Review] [Agents ●2] [Submit review] [⤢]

It reads `Agents`, with a count while runs are running or queued
across the app, and a dot in the accent colour when any run is waiting
on an approval. This count replaces the status bar's Agents control,
which is removed.

Clicking it opens a dropdown under it, anchored as Submit review's is
(absolute, 34 below, right-aligned, z 100, 430 wide). Opening either
popover closes the other and the review launcher. The dropdown shows:

- **Active**: every running, queued or waiting run, grouped by item,
  with the item's title as the group heading. The rows are compact:
  status dot, subject, `Work · Claude`, elapsed or queue position.
- **Recent**: the 15 most recently finished runs across the app, each
  with its item's title and its outcome.
- With no runs at all: `No agent runs yet`.

Clicking a row closes the dropdown and goes to the run: its item opens
(`begin_agent_navigation` as `navigate_to_source` does now), the item
switches to Conversation, the Agents row unfolds, the run expands, and
the page scrolls so the run's row is at the top. A review attempt
expands its group too. A run whose item is gone shows `Source
unavailable` and isn't clickable, as now.

Cmd-Shift-A opens and closes the dropdown from anywhere. Escape closes
it and gives focus back to what had it. Arrow keys move through the
rows and Enter goes to one.

### A local worktree

A branch of the reader's own, or one New work made, has the same two
modes. It always has a checkout, so the switch is always there. Its
Conversation is the same column with the GitHub parts absent: the
summary is Branch, Repository (absent with no GitHub remote), Worktree,
Work may and In Perch, and the cards are its local threads. The Agents
row goes after Branch, where Checks would be. Work runs mostly happen
here, so this is where the row matters most. The bar has no primary
action and no GitHub button, so the Agents button sits just before
expand.

### With nothing open

The empty pane gets a bar like an item's, with no title and only the
Agents button at its right end, where it sits on an item. It keeps the
traffic lights and the sidebar toggle when the sidebar is shut, as the
bare bar does today. The dropdown works as on an item.

### Other ways in

- The completion toast's **View** goes to the run, as a dropdown row
  does.
- The review launcher, while a review is running, unfolds the Agents
  row and scrolls to the review's group, instead of opening the
  inspector.
- An approval arriving for a run on the open item needs nothing new:
  its card is in the thread, and the Agents row's pill shows `waiting`.

## Model

- `ItemTab` becomes `{Conversation, Code}`. `Files` is renamed.
- `ReviewState::conversation_open`, `close_conversation`, and the
  `Pane.conversation` field go. `show_tab(Code)` with no file tabs
  opens the first changed file.
- `Pane` gains `agents_unfolded: bool` and `expanded_runs:
  HashSet<String>`, remembered with the rest of the pane.
- `ReviewState` gains `pending_run_reveal: RwSignal<Option<String>>`,
  set by navigation and consumed by the Agents row once the run's row
  is laid out, as `focus_card` is.
- `AppState`: `drawer: RwSignal<Option<DrawerView>>` becomes
  `tree_open: RwSignal<bool>`. `DrawerView` goes. `agents_tab`, the
  three filters, `selected_agent_run`, `agents_control`,
  `pending_agent_row_focus` and `drawer_opener`'s Agents use go.
  `agents_dropdown_open` and `agents_dropdown_opener` come in, following
  `submit_review_open` and `submit_review_opener`.
- `ui/agents/model.rs` keeps its grouping. `active_groups_with_reviews`
  and `history_runs_with_reviews` take an item id instead of the
  filters. The filter enums and their tests go.

## Backend

- An item's run history, paged: `load_item_runs(item_id,
  before: Option<started_at>, limit)`, backed by a cache query on
  `agent_runs` by item owner. Today's history is app-wide and paged
  100 at a time, and the item's list can't wait on paging through
  everyone else's runs.
- The recent list for the dropdown: `load_agent_runs(15)`, the existing
  call with a smaller page.
- Run detail and events load as now, keyed by run id, when a row
  expands. Several can be expanded at once, which already works:
  `agent_run_events`, `agent_run_events_has_more` and
  `agent_run_events_loading` are keyed by run.

## Harness

Headless tests in the style of the ones they replace.

- Header: the switch is centred within 1px, present with a checkout and
  absent without one. The trailing order is GitHub, Review, Agents,
  primary action, expand. The Agents dropdown opens over the item,
  closes on Escape and gives focus back, and closes Submit review when
  it opens (and the other way round).
- Modes: Cmd-Shift-1/2 switch, Cmd-W on Conversation does nothing,
  Cmd-]/[ skip Conversation, Code with no tabs opens the first changed
  file, closing the last tab stays on Code, and the mode is remembered
  per item across opening another.
- Tree: its tile shows only on Code, Cmd-Shift-E from Conversation
  switches to Code with it open, it docks and overlays as now, and a
  saved `agents` reads as shut.
- Agents row: absent with no runs, the pill's wording for each mix of
  states, folding remembered per item, a review's attempts grouped,
  expand and collapse, two expanded at once, the capped height,
  following the bottom while streaming, Load older runs, and Show in
  thread focusing the card.
- Navigation: a dropdown row for another item opens it on Conversation
  with the row unfolded and the run expanded and at the top. The same
  for the open item, for a review attempt, from the toast's View, and
  from Cmd-Shift-A then Enter. A row with no source does nothing.
- Empty state: the bar with the Agents button, and the dropdown working
  with nothing open.
- Backend: the item history query pages by `started_at` and returns
  only that item's runs, review attempts included.

Tests that go or are rewritten, not ported:
`conversation_is_a_tab_that_the_chat_tile_shows_and_opens_again`,
`cmd_w_on_conversation_closes_it_and_leaves_the_files_tab`,
`conversation_closes_and_opens_again_as_a_tab_does`,
`the_drawer_tiles_open_swap_and_shut_the_drawer_beside_the_pane`,
`the_drawer_keys_open_swap_and_shut_the_drawer_around_the_editor`,
`the_keyboard_opens_the_inspector_which_filters_streams_and_preserves_the_workspace`,
`the_inspector_fills_the_drawer_docked_and_over_the_pane`, the filter
tests in `ui/agents/model.rs`, and `the_bar_is_the_title_and_four_controls_in_a_row`.
`find.rs`'s tests that use `agents-control` move to the Agents button.

## Build order

1. The switch and Code. Restore `switch.rs` with Conversation and Code,
   rename `ItemTab::Files`, drop `conversation_open`, the chat tab and
   the chat tile. The Code no-tabs rule, the keys, pane memory. Leave
   the drawer as it is for this step, agents included. Commit.
2. The Agents row. The item history query, the summary row and pill,
   the unfolded list reusing `list.rs`'s rows and `detail.rs`'s
   sections, expansion and pane memory, Show in thread. Commit.
3. The Agents button and dropdown, on items and on the empty pane,
   navigation to a run, Cmd-Shift-A, the toast and the review launcher
   pointed at the row. Remove the status bar control. Commit.
4. The drawer becomes the tree. Remove `DrawerView`, the agents tile,
   the inspector's drawer panel, its filters and state. Commit.
5. README: the modes and their keys, the Agents row, the Agents button,
   the drawer section rewritten as the file tree, and the removed
   status bar control. Commit.

Step 3 lands before step 4 so there's never a build with no way to see
another item's runs.

## Acceptance

- Open a pull request with a checkout. The bar shows Conversation and
  Code centred. Code shows the first changed file. Close every file
  tab: Code stays, showing `No changes` or the tree, not Conversation.
  Open another item and come back: it's on the mode it was left in.
- Open an issue with no checkout: no switch. Start work on it: the
  switch appears.
- On Conversation, Cmd-W does nothing and the tree tile isn't there.
  Cmd-Shift-E goes to Code with the tree open.
- Ask a question from the composer. The Agents row appears reading `1
  running`. Unfold it and expand the run: the activity streams and
  follows the bottom. When it finishes the pill reads `1 run`, and
  Show in thread flashes the thread card.
- Start a Work run on item A, then open item B. The bar's Agents button
  shows `1`. Open the dropdown: A's run is under A's title. Click it:
  A opens on Conversation with its row unfolded and the run expanded
  at the top.
- Make a worktree with New work and send a Work run. The switch shows,
  the summary's Agents row follows Branch, and the bar's Agents button
  sits before expand with nothing but Review beside it.
- Close every item. The empty pane's bar has the Agents button, and
  the dropdown goes to a run from there.
- Run a review with two backends. The Agents row groups both attempts
  under the review. REVIEW HISTORY still shows the group card.
- A Work run that asks for approval: the pill reads `1 waiting`, and
  the Agents button shows its dot. Answer the card in the thread: both
  clear.
- Relaunch Perch after quitting with the drawer on Agents: it opens
  shut.

## Things the implementer should not do

- Don't keep the inspector as a hidden drawer view "just in case".
  Remove it, its filters and its state.
- Don't put Conversation back in the tab strip or give it a close
  button.
- Don't change the run block in the thread or the approval card.
- Don't page through the app-wide history to build an item's list. Add
  the item query.
- Don't run `cargo fmt --all`. Format only new files.
