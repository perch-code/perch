# Perch: Milestone 3, Saved and Done

## Goal

Make the two inert sidebar rows real. Saved is a bookmark. Done removes a
thread from the inbox until something new happens on it. Both live in Perch
and nowhere else.

This is the first milestone where Perch holds state that is not a mirror of
GitHub. It has to hold it carefully, so that the inbox still feels like a
view over the API and not a second copy that drifts.

## Non-goals

- Syncing with github.com. `DELETE /notifications/threads/{id}` does mark a
  thread done on the web, but nothing lists done threads back, and Saved has
  no API at all. Read and write both stay local. Write-through for Done can
  be a later opt-in.
- Mark as read or unread. Different feature, different endpoint.
- Undo toasts. Undo is opening the Done view and restoring the row.
- Any change to the review view beyond one button.

## Behaviour

Modelled on the web inbox so the two feel the same.

**Saved** is a flag. Toggle it on, the thread appears in the Saved view.
Toggle it off, it leaves. New activity on the thread does not change it. A
saved thread still shows in the inbox.

**Done** hides a thread from the inbox. It moves to the Done view, where it
can be restored. When the thread's `updated_at` moves past the moment it was
marked done, the mark clears on its own and the thread comes back to the
inbox unread or read as the API says. That is what the web does when someone
comments on a PR you dismissed.

A thread can be both saved and done.

Rows can be selected with a checkbox and acted on together, and each
repository group has a Mark as done button for everything in it, the way
the web inbox does.

Filters, search, sort, and grouping apply in every view. The Inbox row's
unread badge counts unread threads that are not done. Saved and Done rows
carry their own counts.

Threads the API stops returning, which happens after a few months, stay in
Saved and Done. Perch keeps its own copy of any marked thread.

## Storage

One table beside `threads` in `perch.db`:

```sql
CREATE TABLE IF NOT EXISTS marks (
    thread_id  TEXT PRIMARY KEY,
    saved      INTEGER NOT NULL DEFAULT 0,
    done_at    TEXT,               -- thread.updated_at when marked, NULL if not done
    marked_at  TEXT NOT NULL,      -- wall clock, for ordering and debugging
    json       TEXT NOT NULL       -- the Thread as it was when last marked
);
```

One row per thread, two independent flags. A row with `saved = 0` and
`done_at IS NULL` is deleted, not kept.

`done_at` holds the thread's own `updated_at` rather than a boolean. The
resurfacing rule is then one comparison: if the API returns the thread with
`updated_at > done_at`, clear `done_at`.

`json` is a snapshot. `store_threads` keeps deleting whatever the API did
not return, so `threads` stays an exact mirror. Marked threads that have
aged out of the API are served from the snapshot instead. When the API does
return a marked thread, the live row wins and the snapshot is refreshed.

The README says deleting the data directory only costs a cold start. After
this milestone it also costs your marks. Update the README rather than
splitting storage into a second file.

## Model

In `perch-github`:

```rust
pub struct Mark {
    pub thread_id: String,
    pub saved: bool,
    /// The thread's `updated_at` at the moment it was marked done.
    pub done_at: Option<Timestamp>,
    pub marked_at: Timestamp,
    pub snapshot: Thread,
}
```

Cache methods:

- `load_marks() -> Vec<Mark>`
- `set_saved(threads: &[&Thread], saved: bool)`
- `set_done(threads: &[&Thread], done: bool)`
- `reconcile_marks(threads: &[Thread]) -> bool`: run after `store_threads`.
  Clears `done_at` where the live thread is newer, refreshes snapshots for
  threads the API still returns. Returns whether anything changed.

All four are plain SQLite and testable without a window or a network.

## Backend

Two commands, each taking a batch:

```rust
SetSaved { thread_ids: Vec<String>, saved: bool },
SetDone  { thread_ids: Vec<String>, done: bool },
```

A single row is a batch of one. A batch is one SQLite transaction, so
selecting a hundred rows and marking them done costs one write and one
event, and a crash mid-way leaves either all or none marked.

One event:

```rust
Marks(Vec<Mark>),
```

The event carries the whole set every time. The set is small, and a full
replacement is simpler than incremental patches on the UI side.

Sequence:

- Startup: send `Marks` alongside the cached `Threads`, before the network
  answers, so the Saved and Done views paint cold like the inbox does.
- `SetSaved` and `SetDone`: write the batch in one transaction, then emit
  `Marks` once. The threads to snapshot come from the backend's last known
  inbox, falling back to the existing snapshot for a thread the API no
  longer returns. An id that matches neither is skipped and logged.
- After every `store_threads`: `reconcile_marks`, and emit `Marks` if it
  changed.

Marking touches nothing on GitHub, so `Last-Modified` does not move and an
unchanged inbox still costs one 304.

## State and filtering

`AppState` gains:

```rust
pub marks:    RwSignal<HashMap<String, Mark>>,
pub view:     RwSignal<View>,
pub selected: RwSignal<HashSet<String>>,
```

with

```rust
pub enum View { Inbox, Saved, Done }
```

`Filters` gains `view`, checked first in `accepts`:

- Inbox: not done.
- Saved: saved.
- Done: done.

Reason, repo, unread, and search stack on top as before.

`selected` holds thread ids. It is cleared when the view changes, when a
filter changes, and after any bulk action. It is not cleared by a poll:
rows that leave the list on refresh are pruned from it, rows that stay keep
their check. Losing a selection because the inbox ticked would be
maddening.

The list the filters run over is no longer `threads` alone. Add
`AppState::pool()`: every live thread, plus the snapshot of every marked
thread the live list does not contain. `visible` and `counts` both read the
pool.

`Counts` gains `saved` and `done`. `unread` becomes unread and not done.

`clear_filters` resets `view` to Inbox and empties `selected`, so the
existing Inbox row click already does the right thing.

Helpers on `AppState`:

- `toggle_selected(id)`, `select_all_visible()`, `clear_selection()`.
- `selection_targets() -> Vec<String>`: the selection if non-empty,
  otherwise empty. The keyboard handler uses this to decide between acting
  on the selection or the row under the pointer.

## UI

### Sidebar

The Saved and Done rows become enabled, selected when `view` matches, with
badges from `Counts`. Clicking one sets `view`. Clicking Inbox returns.

### Rows

A checkbox at the left of each row, after the unread dot and before the
state icon, where the web puts it. Always visible, not hover-only, so a
half-made selection stays legible. Clicking it toggles selection and stops
propagation. Shift-click extends the selection from the last toggled row
through this one, within the visible list order.

Two icon buttons at the right of each row, before the reason pill, visible
on hover. In the Inbox and Saved views: a bookmark that toggles saved, and a
check that marks done. In the Done view the check becomes a restore arrow.
The bookmark is filled when the thread is saved so it reads in every view.

Clicking either stops propagation so it does not open the PR.

Keys, matching the web: `e` marks done, `s` toggles saved. When the
selection is non-empty they act on the selection. Otherwise they act on
the row under the pointer. Escape clears the selection.

### Selection bar

A bar above the list, in the slot where the web shows Select all. Always
present so the layout does not jump. Left: a tri-state checkbox (none, some,
all of the visible rows) with the label "Select all", or "12 selected" once
anything is checked. Right, only while something is selected: **Mark as
done** and **Save** in the Inbox view, **Restore** and **Save** in the Done
view, **Unsave** and **Mark as done** in the Saved view. Save flips to
Unsave when every selected row is already saved.

Select all means every row the filters currently show, across every group,
not just what is scrolled into view.

### Group headers

Each repository section header gets a **Mark as done** button at the right,
like the web. It acts on every visible row in that group regardless of
selection. In the Done view it reads **Restore**. Date groups get the same
button. The single "all" section in Group: None does not, since the
selection bar covers it.

Marking done from the inbox removes the row on the next frame, since
`visible` re-evaluates. No animation.

### Review view

A **Done** button in the header, next to back. Marks the thread done and
closes the review. This is the reason the feature exists: finish a review,
dismiss it, land on the next one.

The review view knows the PR but not the thread. Carry the thread id through
`OpenPullRequest` so the button can find it.

### Empty states

Saved: "Nothing saved. Bookmark a notification to keep it here."
Done: "Nothing done yet."

## Threading

Unchanged. Marks are written on the backend runtime like everything else in
the cache, and the UI only ever sees them through `Event::Marks`.

## Build order

1. `perch-github`: the `marks` table, `Mark`, the four cache methods.
   Tests: set and clear each flag, snapshot survives `store_threads`
   dropping the thread, reconcile clears done on newer `updated_at` and
   leaves saved alone, a row with neither flag is removed. Commit.
2. Backend: commands, event, startup send, reconcile after store. Commit.
3. `View`, `pool`, filter and count changes, with tests in `filter.rs`.
   Commit.
4. Sidebar rows live with badges. Empty states. Commit.
5. Row hover buttons and the `e` and `s` keys, single row only. Commit.
6. Selection: checkbox per row, `selected` on state, the selection bar,
   shift-click, Escape. Keys act on the selection when it is non-empty.
   Commit.
7. Group header buttons. Commit.
8. Review header Done button. Commit.
9. README and the stale comments in `cache.rs`, `client.rs`, and
   `sidebar.rs` that say the API makes this impossible. Commit.

Steps 1 and 3 are independent. Step 6 depends on 5 and the batch shape from
step 2, so build the commands as batches from the start rather than
retrofitting.

## Acceptance

- Mark a PR done from the inbox: the row disappears, the Inbox badge drops
  if it was unread, the Done badge rises, the Done view shows it. Restart
  Perch: still done.
- Restore it from Done: it returns to the inbox in its original position.
- Mark a PR done, then comment on it from the web. On the next poll it is
  back in the inbox, and gone from Done.
- Save a thread, mark it done, comment on it: it returns to the inbox and is
  still saved.
- Delete the `threads` rows from `perch.db` by hand, restart offline: the
  Saved and Done views still list their threads from the snapshots.
- Open a PR, press Done in the header: back at the inbox, the PR is gone
  from it.
- Filter Done by repository or search: works like the inbox.
- Check three rows across two repositories, press Mark as done in the bar:
  all three leave the inbox in one frame, one `Marks` event in the log, the
  selection is empty afterwards.
- Check a row, wait for a poll that changes nothing else: still checked.
- Check a row, change the repository filter: unchecked.
- Shift-click selects the range between the last toggled row and this one.
- Select all with a search active, then Mark as done: only the matching
  rows go. Rows hidden by the search are untouched.
- Press Mark as done on the slatedb group header with nothing selected:
  every visible slatedb row goes, other repositories are untouched.
- Select five rows and press `e`: all five go. With nothing selected, `e`
  acts on the row under the pointer.
- Poll with no inbox changes still returns 304, verified in the log.
- `cargo test` green, `cargo clippy` clean.

## Things the implementer should not do

- Do not call `DELETE /notifications/threads/{id}` or any other write
  endpoint. Nothing in this milestone talks to GitHub.
- Do not stop `store_threads` deleting absent threads. The snapshot column
  exists so that `threads` can stay a mirror.
- Do not write bulk marks as N single writes. One transaction, one event.
- Do not make hover buttons act on the selection. Hover buttons are for the
  one row they sit on; the bar and the keys are for the selection.
- Do not persist the selection across restarts.
- Do not add a settings screen or a config key. There is nothing to
  configure.
- Do not put marks in a second database file. Change the README instead.
- Do not block the UI thread on a mark. It is one SQLite write, but it goes
  through the backend like every other write.
