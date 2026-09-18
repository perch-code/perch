# Perch: Milestone 10, One list

## Goal

Remove the inbox. The sidebar is the only list, and everything the
inbox did that was worth keeping is done there.

After Milestone 9 every open pull request and issue in a declared
repository is a row in the sidebar, and a notification is a reason on
one of those rows. The inbox, which lists notifications that are not
about any item, is left listing subscribed activity in repositories
the reader did not declare, CI runs on default branches, discussions,
releases and security alerts. None of that is what Perch is for, and
the pane, its toolbar, its three views, its five reason filters, its
repository filter, its checkboxes and its bell all exist to serve it.

After this milestone the bell is gone, `Cmd-2` is gone, and the window
has two states: the sidebar with an empty main area, and the sidebar
with an item open. The sidebar's filter menu gains what the inbox had
that applies to items: Saved, Done, needs attention, the reason, and a
sort direction. The notification poll stays, because it is how
attention arrives and it is cheap. The thread cache and the thread marks
go, because nothing reads them.

This is a deletion milestone. The measure of it is how much leaves.

## Non-goals

- Anything the inbox showed that is not a pull request or an issue.
  Discussions, releases, commits and security alerts have no row and
  get none. A CI run on a default branch is a later milestone if it is
  wanted.
- Bulk selection. Checking rows and acting on them together was an
  inbox feature and does not come to the sidebar. `e` and `s` on the
  row under the pointer, and Done on the item header, are the ways to
  mark.
- A per-repository filter. The project sections are that.
- Unread as a state separate from attention. An item needs attention
  or it does not. "Mark unread" already sets attention with reason
  `manual`, and that is what unread means here.
- Changing what makes attention. Milestone 9's table stands.
- A migration. See Storage.

## Behaviour

### The window

`Screen` goes. The window shows the sidebar and, over its main area,
either the item view or the empty state that is there today. `Cmd-[`
from an item lands on the sidebar. `Cmd-2` does nothing and leaves the
keys table.

The sidebar's top is the search row and the filter button, then
Priority, Pinned and the sections. The bell row between the search and
Priority is gone. The collapsed sidebar loses its bell glyph.

### The filter menu

The menu is what it is today with these changes:

- **Status** gains `Done` between `Closed` and `Archived`. `Done` shows
  items that are done and not archived: `done_at` set and no attention
  newer than it. `Closed` no longer includes them; it is items whose
  subject is closed or merged, not done, not archived. `Active` and
  `All` are unchanged. `Archived` is unchanged.
- **Saved only**, a check item after `Kind`. On, the list is narrowed to
  `saved` items across whatever status is set. Priority and Pinned
  narrow too, since a saved filter that left Priority unsaved would
  say two things at once.
- **Needs attention**, a check item after Saved only. On, the sections
  show only items with attention newer than their Done, which is what
  Priority shows, so the sections become Priority grouped. Priority
  itself is unchanged under it.
- **Reason**, a submenu after Needs attention: `Any`, `Review
  requested`, `Mentioned`, `Assigned`, `Commented`, `CI failed`,
  `Manual`. One at a time. An item matches on its `attention_reason`,
  which is the latest reason it was given, done or not. `Any` is the
  default and the submenu is not drawn bold when it is.
- **Sort By** gains a separator and `Newest first` and `Oldest first`
  under the three sorts, one checked. Oldest first reverses the order
  within every section and the order of date sections under Group by
  Date. Priority and Pinned keep their own order, which is not the sort.

The search box is unchanged. A search narrows within whatever the menu
has set, as today.

Every new setting is kept in `item_filters` in `meta`, the way the
existing ones are, and comes back at launch. `perch --items` takes the
new keys.

### Rows

Unchanged. Save and Done on hover, the menu, the attention dot and its
label. A row's Done and Save write the item, as today. The thread marks
they used to write for an inbox row are gone with the inbox.

### The status bar

The poll status is what it was: the time of the last poll and the
source of the token. The inbox count that the bell carried is not shown
anywhere; the Priority section's count is the number that matters.

The backfill line from Milestone 9 is unchanged.

## Model

Gone from `perch-app`:

- `filter.rs` entire: `View`, `ReasonFilter`, `Sort`, `Group`,
  `Filters`, `Section`, `Counts`, `visible`, `sections`, `counts`,
  `view_total`, and their tests.
- `ui/list.rs`, `ui/row.rs`, `ui/toolbar.rs` entire, and `MarkAction`.
- From `ui/sidebar.rs`: `bell_row`, `inbox_nav`, `back_row`,
  `views_section`, `view_row`, `filters_section`, `icon_for`,
  `repos_section`, `repo_row`, and the `Screen::Inbox` branch of
  `sidebar`.
- From `state.rs`: `Screen`, and the fields `threads`, `details`,
  `marks`, `screen`, `return_to`, `view`, `reason_filter`,
  `repo_filter`, `unread_only`, `search`, `sort`, `group`, `loaded`,
  `hovered`, `selected`, `last_toggled`, and every method over them:
  `pool`, `live_ids`, `filters`, `visible`, `sections`, `counts`,
  `view_total`, `is_saved`, `is_done`, `watch_filters`,
  `clear_selection`, `toggle_selected`, the shift-click range,
  `show_screen`.
- From `backend.rs`: `MarkCommand`, `spawn_mark_writer`, `write_marks`,
  `resolve_threads`, `send_marks`, `fetch_details`, `Event::Threads`,
  `Event::Detail`, `Event::Marks`, `Backend::set_saved`,
  `Backend::set_done`, and the `LAST_MODIFIED_KEY` conditional on the
  notification poll, which was there so an unchanged inbox cost one
  cheap 304 and is worth keeping only as long as the threads are
  stored. They are not. The poll fetches the first page each time; it
  is one request and `X-Poll-Interval` still paces it.
- From `icons.rs`: the bell, the inbox, and whatever else only the
  inbox drew. Check each glyph for a remaining caller before removing.

Gone from `perch-github`:

- `Mark`, `Cache::store_threads`, `load_threads`, `reconcile_marks`,
  `set_saved`, `set_done`, `marks`, `store_detail`, `load_details`.
- `Thread` stays. `attend` and `refresh_open_checks` read it, and
  `list_threads` returns it. `SubjectDetail` goes if nothing but the
  inbox read it; `fetch_detail` with it.

Changed:

- `ItemStatus` gains `Done`, with `accepts`, `key`, `label`, and the
  `Closed` arm narrowed.
- `ItemFilters` gains `saved_only: bool`, `attention_only: bool`,
  `reason: Option<AttentionReason>`, `direction: Direction`, with
  `to_meta` and `parse` keys `saved`, `attention`, `reason`,
  `direction`.
- `AttentionReason`, an enum over the strings `attention_reason` holds:
  `ReviewRequested`, `Mention`, `Assign`, `Comment`, `CiFailed`,
  `Manual`, with `label` and the `from_str` that `attention_label`
  does by hand today; `attention_label` becomes a call on it. A
  `team_mention` is `Mention`. Anything else is `None` and matches only
  `Any`.
- `items::sections` takes the four new settings. `saved_only` and
  `attention_only` filter the searched set before Priority and Pinned
  are cut from it; `reason` likewise; `direction` reverses `rows` after
  `sort_items` and the bucket order in `by_date`.

## Storage

`threads`, `subject_details` and `marks` are dropped. The
`notifications_last_modified` key in `meta` is not read and is left to
be dropped with the database.

The schema version bumps to 11 and the database is recreated, as every
schema change so far has done. Milestone 8 said the next change would
write a migration. It does not. Nobody is running Perch against a
database they need to keep, and a clean model is worth more than a
migration nobody would exercise. Items, threads with agents, marks,
viewed sets and discussions are lost on first launch; the list pass and
the backfill from Milestone 9 rebuild the items and the discussions
within a poll, and knowledge on disk is untouched. Say so in the commit
message.

## Backend

- The poll loop keeps the notification poll and its `attend`. `refresh`
  loses the cache write, the mark reconcile, the `Threads` event and
  `fetch_details`. What is left: list, attend, status. Avatars are
  still wanted for the discussion's authors and are fetched from there
  as today; the inbox was the other caller and there is none now.
- `Backend::mark_item` is the only mark path.
- `Command::OpenSubject` goes. It was how an inbox row opened. Every
  row is an item and opens by id.

## UI

`ui/mod.rs`:

- `app_view` builds the sidebar and the review over it. The
  `dyn_container` that switched on `Screen` goes.
- The key handler loses the inbox branches: `Cmd-2`, the checkbox
  range, `Escape` dropping a selection, `e` and `s` over a thread. `e`
  and `s` over an item row stay. `Escape` still puts away a rename or a
  confirmation.
- `key_mark` and everything that took a thread id go.

`ui/sidebar.rs`:

- `sidebar` is `projects_nav` alone. The `Screen::Inbox` alternative
  and its `dyn_container` go.
- `filter_menu` gains the entries above. The reason submenu is built
  the way Status is, one item per variant plus `Any`, radio-checked.
  Direction is two radio items under a separator in Sort By.

`ui/status.rs`: nothing to remove; the count came from the sidebar.

`review/`: `return_to` was read where the header's back button and
`Cmd-[` decided where to land. They land on the sidebar; the read goes.

## Harness

`perch --items` takes `saved=1`, `attention=1`, `reason=mention`,
`direction=oldest`, and prints what the window shows. `--dump-file`,
`--scan`, `--timeline`, `--scope`, `--sync` and `--backfill` are
unchanged.

## Build order

1. `perch-app::items`: `ItemStatus::Done`, `AttentionReason`, the four
   settings on `ItemFilters`, `to_meta` and `parse`, and `sections`
   over them. Tests: `Done` and `Closed` do not overlap; saved only
   narrows Priority; attention only leaves Priority alone; a reason
   matches the latest reason and `team_mention` is `Mention`; oldest
   first reverses rows and date buckets and not Priority. `perch
   --items`. Commit.
2. `ui/sidebar.rs`: the menu entries. The window shows the new filters
   over the existing list. Commit.
3. The deletion: `filter.rs`, the three inbox view files, the sidebar
   sections, the state fields, `Screen`, the backend events and
   commands, the mark writer, the cache methods, the icons, `Cmd-2`,
   `return_to`. `cargo build` is the guide: remove `Screen`, follow the
   errors until none. One commit, so it can be read as one change and
   reverted as one.
4. `perch-github`: drop the three tables, bump to 11. Commit, saying
   what is lost.
5. README: the inbox section goes; its Saved and Done bullets move
   under Projects and items and say they are item marks; the keys table
   loses `Cmd-2` and the words "or the inbox"; "What it deliberately
   does not do" keeps the paragraph on marks not syncing, reworded for
   items. The knowledge section is Milestone 9's. Commit.

Step 1 is independent of everything after Milestone 9's step 4 and can
land first. Steps 3 and 4 are one change in two commits.

## Acceptance

- Launch. No bell, no inbox, `Cmd-2` does nothing. The sidebar is the
  first thing and the only thing. The status bar shows the poll and
  nothing about a count.
- Be mentioned on an issue. Within a poll Priority shows it with
  "mentioned". Press `e` over it; it leaves Priority. Set Status to
  Done; it is listed. Set Status to Closed; it is not.
- Set Reason to Review requested. Only items whose latest reason is a
  review request are listed, under their projects, and Priority shows
  only those too. Set Any; everything returns.
- Save an item, set Saved only. The list is that item, and Priority and
  Pinned show only saved items. Turn it off; everything returns.
- Set Needs attention. The sections hold exactly the Priority items,
  grouped by project. Group by Date; grouped by date. Turn it off.
- Sort by Last activity, Oldest first. The bottom row of each section
  is the newest, and under Group by Date the Older bucket is first.
  Quit and relaunch; the direction held.
- `perch --items "status=done;reason=mention;direction=oldest"` prints
  what the window shows with those settings.
- Open an item, `Cmd-[`. The sidebar, with the item's row still
  selected. There is no other place to land.
- `grep -r Inbox crates/` finds nothing but the README's history and
  the milestone files. `grep -r Thread crates/perch-app` finds only
  `attend`, `refresh_open_checks` and the comment thread types.
- Open the database. Three tables fewer, version 11.
- `cargo test` green, `cargo clippy` clean, and the warning count for
  unused code is zero, because the deletion followed every error to the
  end.

## Things the implementer should not do

- Do not keep the thread cache "in case". The notification poll's
  result lives for the length of `attend` and is dropped. If it is
  wanted later it is one request away.
- Do not keep `Screen` with one variant. Remove it.
- Do not move the reason filters into the sidebar as rows. They were
  rows in the inbox because the inbox had a nav column. The sidebar's
  nav is the project list, and the filters are a menu.
- Do not add a repository filter. Sections.
- Do not add checkboxes to item rows. Bulk marking is not coming back.
- Do not make `Done` a status that hides the item everywhere. It is a
  status like Closed: shown when asked for.
- Do not filter Priority by reason and leave the sections alone, or the
  other way round. One list, one narrowing.
- Do not let the direction touch Priority or Pinned. Attention order
  and pin order are not the sort.
- Do not write a migration. Bump and recreate, and say what is lost.
- Do not leave a dead icon, a dead event, a dead command, a dead cache
  method. The build order says follow the errors. Then look for what
  compiled anyway: `cargo build` with warnings on, and read them.
- Do not remove the notification poll, `attend`, `refresh_open_checks`
  or `Thread`. They are how attention arrives.
- Do not rename `item_filters` in `meta` or change the existing keys.
  Add four.
