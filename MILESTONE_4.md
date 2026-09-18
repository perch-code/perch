# Perch: Milestone 4, Worktrees

## Goal

Make the worktree the thing Perch works on. Today a worktree is a side
effect of clicking a pull request: it appears under the data directory,
keyed by commit, and a sweep deletes it two weeks later. Nothing in the UI
knows it exists.

After this milestone a worktree is a record with a name, a list to find it
in, and a lifecycle the user controls. Opening one shows the review view.
Archiving one frees the disk and keeps the record, so restoring it is a
checkout, not a clone. A pull request is one way to make a worktree, and
for now the only way, but the record does not assume it.

The long-term plan is for external tools, a coding agent's CLI in
particular, to change files inside a worktree while Perch shows them. That
is not this milestone. This milestone makes sure the worktree is shaped so
that plan does not need a rewrite: it sits on a branch, it is keyed by that
branch rather than by a commit, and it is never deleted with work in it.

## Non-goals

- Editing in Perch. The editor stays read-only.
- Worktrees without a pull request. The record allows it; no UI creates it.
- Repositories that are not on GitHub. The record's repository field is
  still `owner/repo`; widening it to a URL or a local path is its own
  milestone.
- Fork remotes, ahead/behind, pushing. Nothing in this milestone runs a
  command that touches the remote other than fetch.
- Watching the filesystem. The review view reads the worktree once on open,
  as it does now.
- Deleting bare clones. The store's `repos/` directory only grows. A
  "forget repository" action needs every worktree of that repository
  archived first, and can wait.
- Migrating the commit-keyed worktrees from earlier builds. They are
  removed on first launch. Nothing in them is the user's.

## Behaviour

A worktree is one directory under `<data dir>/worktrees/`, checked out
from the bare clone of its repository, on a local branch Perch owns. For a
pull request the branch is `perch/pr-<number>`, pointing at the pull
request's head. The branch exists so that a commit made in the worktree by
something other than Perch has somewhere to land. Nothing in this milestone
makes such a commit.

Clicking a pull request in the inbox opens its worktree, creating it if
there is none. Several notifications for the same pull request share one
worktree. Opening the same pull request twice costs a fetch and a diff, not
a checkout.

When the pull request's head has moved since the worktree was made, and the
worktree is clean, Perch resets the branch to the new head before showing
the review. When the worktree is not clean, Perch leaves it alone, shows the
review of what is on disk, and says in the header that the pull request has
moved on. Nothing writes to a worktree in this milestone, so the dirty case
should not arise, but the check is what makes the branch safe to build on
later.

The activity rail gains a Worktrees entry beside the inbox. It lists every
worktree Perch has, newest opened first: repository, pull request number
and title, branch, pull request state, when it was last opened. Clicking a
row opens the review view. The entry is current while the list is showing
and while a review is showing, whichever way the review was reached,
because a review is a worktree.

**Archive** removes the directory and keeps the record, the branch, and the
fetched refs. Archived worktrees show in their own segment of the list.
**Restore** checks the branch out again into the same path and opens it.
Both are a few seconds; neither is a clone.

Archive refuses a worktree with uncommitted changes and says so. This is
the one rule that has to hold once agents write into worktrees, so it holds
from the start.

**Forget** removes an archived record and its branch. It is the only way a
row leaves the list.

The two-week sweep stays, but it archives instead of deleting. A worktree
nobody has opened in fourteen days loses its directory and keeps its row.

From a review, the header names the worktree and offers Reveal in Finder,
Open in Terminal, Copy path, and Archive. Archiving from the header closes
the review and lands on the Worktrees list. Perch owning the directory is
only acceptable if getting to it is one click.

`Cmd-[` from a review returns to wherever the review was opened from: the
inbox, Saved, Done, or the Worktrees list.

## Storage

One table beside `marks` in `perch.db`, for the same reason marks are
there: one file, one cold-start story.

```sql
CREATE TABLE IF NOT EXISTS worktrees (
    id              TEXT PRIMARY KEY,   -- "<owner>/<repo>#<branch>"
    repo_full_name  TEXT NOT NULL,
    branch          TEXT NOT NULL,      -- "perch/pr-123"
    base_ref        TEXT NOT NULL,      -- branch the diff is taken against
    pr_number       INTEGER,            -- NULL for a worktree with no PR yet
    head_sha        TEXT NOT NULL,      -- what the branch pointed at last time Perch looked
    path            TEXT NOT NULL,      -- absolute; where the checkout lives or lived
    created_at      TEXT NOT NULL,
    last_opened_at  TEXT NOT NULL,
    archived_at     TEXT,               -- NULL while the directory exists
    pr_json         TEXT                -- PullRequest snapshot, for the list row
);
```

The key is repository and branch, not pull request number and not commit.
A pull request maps to a branch, so it is findable; a commit is a value the
row carries, so a force-push updates a row instead of making one. The
number is nullable so that a worktree made for some other reason later can
gain a pull request after the fact.

`pr_json` is a snapshot so the list can paint offline and before the
network answers, like the inbox. It is refreshed whenever the pull request
is fetched.

`path` is stored rather than derived, so that the layout on disk can change
without orphaning rows. Today it is
`<data dir>/worktrees/<owner>__<repo>__<branch with / as ->`.

## Model

In `perch-github`, beside `Mark`:

```rust
pub struct Worktree {
    pub id: String,
    pub repo_full_name: String,
    pub branch: String,
    pub base_ref: String,
    pub pr_number: Option<u64>,
    pub head_sha: String,
    pub path: PathBuf,
    pub created_at: Timestamp,
    pub last_opened_at: Timestamp,
    pub archived_at: Option<Timestamp>,
    pub pull_request: Option<PullRequest>,
}

impl Worktree {
    pub fn is_archived(&self) -> bool;
    pub fn for_pull_request(repo: &str, number: u64) -> String;  // the id
}
```

Cache methods, all plain SQLite:

- `load_worktrees() -> Vec<Worktree>`
- `upsert_worktree(&Worktree)`
- `touch_worktree(id, opened_at)`
- `set_worktree_archived(id, Option<Timestamp>)`
- `set_worktree_head(id, sha, pull_request: Option<&PullRequest>)`
- `forget_worktree(id)`

In `perch-git`:

- `Store::worktree_path(id: &RepoId, branch: &str) -> PathBuf` replaces the
  commit-keyed version.
- `Preparer::prepare` takes a branch name and a head, and runs
  `git worktree add -B <branch> <path> <head>` when the directory is
  missing, or `git worktree add <path> <branch>` when the branch already
  exists from an archived row. When the directory exists it fetches, then
  either resets to the new head or reports `Checkout::stale` with the
  head it kept.
- `Preparer::is_dirty(path) -> bool`: `git status --porcelain` non-empty.
- `Preparer::remove_worktree(bare, path)`: `git worktree remove --force`
  is not used; it would delete uncommitted work. Delete the directory
  ourselves after the dirty check, then `git worktree prune`.
- `Preparer::delete_branch(bare, branch)` for Forget.
- `Preparer::stale_worktrees(max_age) -> Vec<PathBuf>` replaces
  `sweep_worktrees`. It reports; the backend decides, because only the
  backend has the records.

`Checkout` gains `branch: String` and `stale: Option<String>`, the head the
pull request moved to while the worktree was dirty.

## Backend

Commands:

```rust
OpenPullRequest { repo_full_name: String, number: u64 },   // as now
OpenWorktree    { id: String },
ArchiveWorktree { id: String },
RestoreWorktree { id: String },
ForgetWorktree  { id: String },
```

Events:

```rust
Worktrees(Vec<Worktree>),
WorktreeFailed { id: String, message: String },
```

`Worktrees` carries the whole set, like `Marks`. It is small.

Sequence:

- Startup: send `Worktrees` from the cache with `Threads` and `Marks`.
  Then reconcile the store with the table: a row whose path is missing
  becomes archived; a directory under `worktrees/` with no row is removed
  and pruned. This is also what retires the commit-keyed layout.
- `OpenPullRequest`: fetch the pull request as now. Find or create the row
  for `perch/pr-<n>`. Prepare with that branch. Update `head_sha`,
  `pr_json`, `last_opened_at`, clear `archived_at`. Emit `Worktrees`, then
  `ReviewReady` carrying the `Worktree` alongside the `PullRequest`.
- `OpenWorktree`: the same, from the row's repository and number. When the
  pull request fetch fails, and the directory exists, open what is on disk
  with the snapshot and a status message. A worktree opens offline.
- `ArchiveWorktree`: refuse and emit `WorktreeFailed` if dirty. Otherwise
  remove the directory, prune, set `archived_at`, emit `Worktrees`. If the
  archived worktree is the open review, drop the review context and stop
  the language server.
- `RestoreWorktree`: clear `archived_at`, then behave as `OpenWorktree`.
- `ForgetWorktree`: only for an archived row. Delete the branch, delete the
  row, emit `Worktrees`.
- Sweep: on the existing timer, for each stale path with a row, do
  `ArchiveWorktree`. A stale path with no row is removed.

The language server is started against the worktree path as now. Nothing
about that changes.

## State

`AppState` gains:

```rust
pub worktrees: RwSignal<Vec<Worktree>>,
pub screen:    RwSignal<Screen>,
pub return_to: RwSignal<Screen>,
```

with

```rust
pub enum Screen { Threads, Worktrees }
```

`Screen::Threads` is the existing inbox with its Inbox, Saved and Done
views; `view` keeps meaning what it means. `Screen::Worktrees` is the new
list. The review view sits over either, as it does now.

`return_to` is set from `screen` whenever a review opens, and read by
`Cmd-[` and the header's back button.

`ReviewState` gains `worktree: RwSignal<Option<Arc<Worktree>>>` and
`stale_head: RwSignal<Option<String>>`.

A `WorktreeSegment { Active, Archived }` signal for the list, defaulting to
Active.

## UI

### Activity rail

A Worktrees entry under Inbox, with a branch glyph. Current when
`screen == Worktrees` or a review is showing. Clicking it leaves a review
if one is open, sets `screen`, and does not touch the inbox's filters or
selection, for the reason in the sidebar's comments.

No badge. A count of worktrees is not something to act on.

### Worktrees list

Uses the inbox's list chrome: same row height, same type scale, the same
empty-state treatment. A segment control at the top, Active and Archived,
styled like the review rail's Changed and All files.

A row, left to right: repository, then pull request number and title on
one line with the branch under it in the meta size, then a state pill
(Open, Draft, Merged, Closed, from the snapshot), then last opened as a
relative time. Archived rows show the archive time instead.

Hover buttons at the right, before the time, like the inbox's: Archive on
an active row; Restore and Forget on an archived row. Clicking one stops
propagation.

Clicking an active row opens it. Clicking an archived row restores and
opens it; the progress card says "Checking out the files" while it does.

Right-click, or a `...` hover button, offers Reveal in Finder, Open in
Terminal, Copy path on active rows.

Empty states. Active: "No worktrees. Open a pull request from the inbox to
make one." Archived: "Nothing archived."

### Review header

After the branch names, the worktree's directory name in the meta colour,
clickable, opening the same Reveal, Terminal, Copy, Archive menu.

When `stale_head` is set: a small notice, "Pull request moved to abc1234;
showing the worktree as it is." No button. Resolving it is a later
milestone's problem, and today it cannot happen.

Back returns to `return_to`.

### Keys

| Key | What it does |
|---|---|
| `Cmd-[` | Back to `return_to` from a review. |
| `Cmd-2` | The Worktrees list. `Cmd-1` becomes the inbox. |

`e` and `s` stay inbox keys. They do nothing on the Worktrees screen.

## Threading

Unchanged. Worktree writes happen on the backend runtime. Archive is disk
I/O and a git prune, so it runs on a spawned task like `OpenPullRequest`,
not inline in the command loop.

## Build order

1. `perch-git`: branch-keyed `worktree_path`, `prepare` on a branch with
   the reset-or-stale logic, `is_dirty`, `remove_worktree`,
   `delete_branch`, `stale_worktrees`. Tests against a temporary bare
   repository: a fresh branch is created, an existing branch is reused
   after the directory is removed, a clean worktree follows a moved head, a
   dirty one does not and reports it. Commit.
2. `perch-github`: the `worktrees` table, `Worktree`, the six cache
   methods. Tests: upsert then load, archive is a nullable timestamp,
   forget removes, snapshot survives with no network. Commit.
3. Backend: commands, events, startup send, store reconciliation, the
   sweep archiving through the same path. `ReviewReady` carries the
   worktree. Commit.
4. `Screen`, `return_to`, `worktrees` on `AppState`. The rail entry and
   `Cmd-1`, `Cmd-2`. An empty Worktrees screen. Commit.
5. The list: rows, segments, open on click, empty states. Commit.
6. Archive, Restore, Forget from the rows, with the dirty refusal surfaced
   in the status line. Commit.
7. Review header: directory name, menu, stale notice, back to
   `return_to`. Commit.
8. README: the review view section, the keys table, and a Worktrees
   section saying where they live and that archiving is safe. Commit.

Steps 1 and 2 are independent. Step 3 needs both.

## Acceptance

- Open a pull request from the inbox. `git -C <bare> branch` lists
  `perch/pr-<n>`; `git worktree list` shows the path on that branch; the
  Worktrees list has one row, Open, opened just now.
- Open a second notification for the same pull request: still one row.
  The log shows a fetch and no checkout.
- Push a new commit to the pull request, open it again: the row's head
  moves, the diff shows the new commit, the directory is the same.
- Touch a file in the worktree by hand, push another commit, open it: the
  header says the pull request moved on, the diff is of what is on disk.
  Revert the file, open again: caught up.
- Archive from the row: the directory is gone, `git worktree list` no
  longer shows it, the branch is still there, the row is under Archived.
- Restore: the directory is back on the same path, the review opens, no
  clone in the log.
- Touch a file, then Archive: refused, the file is still there, the status
  line says why.
- Forget an archived row: the row and the branch are gone.
- Quit, launch offline, open the Worktrees list: rows paint from the
  snapshot. Open one: the review shows from disk with an offline status.
- Delete a worktree directory by hand, launch: its row is Archived.
- Launch a build with the old commit-keyed directories present: they are
  removed and the log says so.
- Set the sweep age to a minute and wait: the worktree is archived, not
  deleted, and its row says so.
- Open a review from Saved, press `Cmd-[`: Saved. From the Worktrees
  list: the Worktrees list.
- Reveal in Finder opens the directory. Open in Terminal lands in it.
- `cargo test` green, `cargo clippy` clean.

## Things the implementer should not do

- Do not key anything by pull request number alone. The id is repository
  and branch; the number is a column.
- Do not use `--detach`. The branch is the point.
- Do not use `git worktree remove --force`. Check for dirt, delete the
  directory, prune.
- Do not delete a worktree from the sweep. The sweep archives.
- Do not auto-archive on merge or close. Show the state; let the user
  decide.
- Do not run `du` for a size column. It would make the list slow to paint
  and answers a question nobody asked yet.
- Do not add a settings screen. The sweep age stays a constant.
- Do not let the Worktrees entry reset inbox filters or selection.
- Do not poll GitHub for pull request state from the list. The snapshot is
  refreshed when a worktree is opened, and that is enough.
- Do not put the table in a second database file.
