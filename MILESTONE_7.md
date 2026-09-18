# Perch: Milestone 7, Projects

## Goal

Make the project the spine of the app, and make everything else a thing
that belongs to one.

Today Perch has two screens, Notifications and Worktrees, reached from a
rail. The inbox is the front door, and a repository is a filter on it. But
three of the six milestones are project-shaped already: worktrees group by
repository, knowledge maps repositories to a project name, and every review
is a checkout of one branch of one repository. The inbox is the only place
where the repository is not the spine, and it is the oldest part.

After this milestone the sidebar lists projects, and under each project the
things being worked on: a pull request someone opened, an issue, a branch
of your own that has no pull request yet. Clicking one opens the view that
fits it. A notification about one of those things is no longer a row of its
own: it sets attention on the item, and a Priority section at the top of
the sidebar lists what needs looking at. The inbox, behind a bell, holds
only what is not an item: discussions, releases, workflow runs, and mentions
in repositories you have not opened. The list has a filter, a sort, a
grouping, and a search, shaped like the ones on the inbox today.

This milestone does not keep old data. `perch.db` from an earlier build is
deleted on first launch and the worktrees in it are re-cloned when next
opened. Threads made before this milestone are lost. That is a one-time
cost paid now, for a schema with no migration in it.

The design decision the milestone turns on is what the thing under a
project is. It is one record, an *item*, with three optional parts: a
GitHub subject, a checkout, and attention. Which parts are present decides
what the item looks like and what opens when it is clicked. Nothing is
stored as "a PR item" or "an issue item". A branch of your own becomes a
pull request when GitHub opens one on it, and it is the same row before and
after.

## Non-goals

- Backwards compatibility. No migration of `perch.db`, no old ids kept, no
  `[[knowledge.project]]` alias. The database is recreated and the config
  key is renamed. Nothing before this milestone has shipped to anyone.
- A settings UI. Projects and their roots are declared in `config.toml`.
  An "Add project" button comes when there is a second thing to put in a
  settings window.
- Writing to GitHub. Starting work on an issue makes a local branch. It
  does not assign the issue, comment on it, or open a pull request.
- Editing in Perch. A local branch is shown, diffed and asked about. It is
  changed in an editor, and Open in Terminal is still how you get there.
- Cloud or remote agents. The Environment filter Claude's sidebar has does
  not apply; there is one environment and it is this machine.
- Every branch in your checkouts. A local item is a git worktree, or the
  branch checked out in a root. The forty stale branches in `refs/heads`
  are not items until they are checked out somewhere.
- GitHub's own issue-to-pull-request linkage. An issue shows the branches
  and pull requests Perch started from it. Reading "Closes #42" out of
  bodies and timelines is later.
- Discussions, releases, commits, and workflow-run notifications as items.
  They stay in the inbox. Only issues and pull requests resolve to items.
- Auto-creating a project from a notification. A mention in a repository
  you have never opened lands in the inbox and nowhere else.
- Mark as read, unsubscribe, or anything the previous milestones said
  Perch does not do to your GitHub account.

## Behaviour

### The sidebar

The drawer on the left is now the whole navigation. Top to bottom:

- A header with the search box and a filter button that opens the menu
  described below.
- A bell with an unread count. Clicking it opens the Inbox: the
  notifications screen as it exists today, with its own filters, showing
  only notifications that resolve to no item. The count is over those.
- **Priority.** Items with attention newer than their Done mark, newest
  attention first. "Nothing needs attention" when empty. This section
  ignores the group and sort settings and respects the search.
- **Pinned.** Items the reader pinned, most recently pinned first, with a
  pin glyph. Hidden when empty. Ignores group and sort like Priority. An
  item in Pinned is not repeated under its project.
- One section per project, named, with a count of its active items in the
  muted colour. Under it, the project's items as rows. A project with no
  items is hidden unless Show empty projects is on.

An item row shows: a state glyph on the left, the title, and a second line
with the item's reference. The glyph and reference depend on what the item
has:

| Item has | Glyph | Reference line |
|---|---|---|
| Pull request | the PR state icon from the inbox | `owner/repo #57 · branch` |
| Issue | the issue state icon | `owner/repo #42` |
| Checkout only | a branch glyph | `owner/repo · branch`, and `+12 −3` against base |

A row with attention shows a dot in the accent colour at its right edge. A
row whose checkout is archived is in the muted colour. Hovering shows the
Save and Done glyphs at the right edge, as the inbox rows do, and the same
keys work: `e` for Done, `s` for Save, on the row under the pointer or the
selected row.

With Show PR status on, a pull request row's reference line ends with a
CI chip (a green check, a red cross, or a yellow dot) and a review chip
("review requested", "changes requested", "approved") when there is one.
Both come from the pull request snapshot Perch already fetches; no new
request is made for the row.

Clicking a row opens its view. `Cmd-[` returns to whatever the sidebar
showed. `Cmd-1` focuses the sidebar search. `Cmd-2` opens the Inbox.

### The filter menu

The button on the sidebar header opens a menu, in the shape of Claude's:

| Row | Options | Default |
|---|---|---|
| Status | Active, Closed, Archived, All | Active |
| Kind | Pull requests, Issues, Branches (multi) | all three |
| Group by | Project, Date | Project |
| Sort by | Last activity, Last opened, Created | Last activity |
| Show empty projects | toggle | off |
| Show PR status | toggle | on |

Active means: not archived, not Done, and the subject is open or absent.
Closed means the subject is closed or merged, or the item is Done. Archived
means the checkout is archived. All is all. Group by Date replaces the
project sections with Today, Yesterday, and the weekday and date sections
the inbox uses, each row carrying its project name in the reference line
instead of `owner/repo`. Last activity is the later of attention and the
subject's `updated_at`; for a checkout-only item it is the head commit's
time.

Search is a substring over title, number, branch name, and project name.
It applies to the Priority section too.

The settings persist in `meta`, like the inbox's do.

### The item menu

Every row has a `...` at its right edge on hover, and a right-click opens
the same menu. It has the shape of Claude's session menu, and every row
gets every entry, with the ones that cannot apply greyed rather than
hidden, so the menu is the same size and order everywhere:

| Entry | Key | What it does |
|---|---|---|
| Open in | | Submenu: GitHub, Finder, Terminal, Copy path. |
| Pin / Unpin | `P` | Toggles the item in the Pinned section. |
| Mark unread | `U` | Sets attention to now with reason `manual`. The item goes to Priority with its dot. Done clears it, as for any attention. |
| Rename | `R` | The title becomes a text box in the row. Enter commits, Escape cancels, empty restores the GitHub title or branch name. |
| Fork | `F` | Cuts a new branch from the item's head and makes a new item on it. |
| Move to project | | Submenu of projects, with the current one checked, and "New project…" at the bottom. |
| Archive / Restore | `A` | Archives the item, or restores an archived one. |
| Delete | `D` | Removes the item. Red. Asks once. |

The letters work while the menu is open. They do not work on a bare row,
where `e` and `s` keep doing what they do on the inbox.

**Open in** shows GitHub for an item with a subject, and Finder, Terminal
and Copy path for an item with a checkout on disk. An issue has only
GitHub; an archived checkout has only GitHub.

**Rename** sets a name of the reader's own that the row shows in place of
the pull request title, the issue title, or the branch name. The reference
line still shows the real number and branch, so the item can be found. The
search matches both.

**Fork** on a pull request or a branch item runs `git worktree add -b
<branch>-2 <path> <head_sha>` under Perch's store, with `-3` and on if that
name is taken, and inserts a checkout-only item with `from_item_id`
pointing at the source. The review view opens on it with an empty diff
against the source's head, so what is shown is what has been done on top.
Fork on an issue is Start work, and the entry reads Start work there. Fork
on an archived item is greyed.

**Move to project** sets the item's project and nothing else. An item can
live in a project that does not claim its repository; the project is a
place, not a rule. "New project…" makes an implicit project with the typed
name, no repositories and no roots, and moves the item into it. Such a
project holds only what is moved there and is deleted when its last item
goes.

**Archive** is an item state. An archived item leaves Active and is found
under the Archived status. When the item has a checkout Perch owns, the
directory is removed too, and refused when there is uncommitted work in
it, as today. A checkout under one of your roots is left alone; the item
is hidden and the root scan does not bring it back while it is archived.
Restore reverses it: the item returns to Active, and an owned checkout is
put back on disk.

**Delete** removes the row and its threads. For an owned checkout it
removes the directory and the branch, and refuses while there is
uncommitted work. For a checkout under a root it touches nothing on disk,
and the root scan will make a fresh item for that worktree on the next
pass unless the worktree is gone; a Delete of a root item is, in effect,
"forget the threads and start over". The confirmation names what will be
removed on disk, if anything. Knowledge files are never deleted.

### Projects

A project is declared in `config.toml`:

```toml
[[project]]
name = "perch"
repos = ["criccomini/perch"]
roots = ["~/Code/perch"]

[[project]]
name = "slatedb"
repos = ["slatedb/slatedb", "slatedb/slatedb-go"]
```

`repos` says which GitHub repositories belong to it. `roots` says where you
already work on it: directories that are git checkouts of one of those
repositories, or worktrees of one. `[[project]]` replaces
`[[knowledge.project]]`, which is no longer read, and `project_for` in the
knowledge crate is now a lookup on the same list.

A project that is not declared appears anyway when an item needs one. Open
a pull request from the inbox in a repository that no project claims, and
an implicit project named after the repository holds it. This is the
fallback the knowledge directory already uses, and the two agree on the
name. An implicit project has no roots. Declaring it in config later
adopts its items.

Each repository in a project has a **home checkout**: a worktree of the
bare clone on the repository's default branch, under Perch's store, made
the first time something needs it. It is not an item and is not listed. It
is where the agent runs when asked about an issue, where a new branch is
cut from when work starts on one, and what the knowledge skill points at
when there is no pull request. It follows the default branch on every
refresh, with a fast-forward, and it is never archived.

### Items with a pull request

What a worktree is today. Opening one prepares the checkout and shows the
review view, unchanged. An item with a pull request and no checkout yet,
which happens when a review request arrives before the pull request is
opened, prepares the checkout on first open, as the inbox does today.

### Items with an issue

Opening one shows the **issue view**: the title, number and state in the
header with the labels and assignees; the body rendered from markdown; the
comments under it in order, each with author, avatar and time; and, under a
divider, the branches and pull requests Perch started from this issue,
each a clickable row. No file rail, no editor.

The thread column works here as it does in a review, with no anchors. `a`
or the `+` at the top of the column opens a composer. The run happens in
the repository's home checkout, read-only, with the issue's title, body,
and comments folded into the prompt in place of the pull request block. The
questions this is for: where in the code does this live, has this come up
before, what would a fix look like. Threads on an issue are stored like
threads on a review and reopen with it.

The header has a **Start work** button. It cuts `perch/issue-<n>` from the
default branch in the repository's bare clone, checks it out under Perch's
store, and creates a new item: a checkout, no subject, with `from_item_id`
pointing at the issue. The review view opens on it with an empty diff. The
issue item is unchanged and now lists the branch under its divider. If the
branch already exists the button opens the item that has it.

Opening an issue archives its discussion under the knowledge root as
`issues/<n>.md` beside `pulls/`, with the same record shape. The skill's
text is extended to say issues are there too.

### Items with a checkout and no subject

A branch of your own. Opening one shows the review view with the diff
taken against `base_ref`, which is the repository's default branch, and a
header that names the branch and the directory and has no pull request
block. Everything else is the same: the file rail, the editor, hover and
definition, threads. When the branch has no commits past its base the diff
is empty and the rail shows the whole tree, which is the state a fresh
Start work branch is in.

These items come from two places.

Perch makes one when Start work is pressed, under its own store, and owns
it: archive and restore work as they do for pull request checkouts.

Perch also finds them in the project's roots. On startup and on every
Refresh, each root is asked `git worktree list --porcelain`, and each
worktree whose branch is not the default branch is an item with that
directory as its checkout and `owned = false`. The root's own checked-out
branch counts. Perch never touches a checkout it did not make: Archive and
Delete on such an item act on the row, never on the directory, as the item
menu section says. When a foreign directory is gone on the next scan, its
row is deleted, unless it has threads, in which case it is archived and
its Restore is greyed.

### A branch becomes a pull request

On every Refresh, for each item that has a branch and no subject, Perch
asks GitHub for pull requests whose head is that branch:

```
GET /repos/{owner}/{repo}/pulls?head={owner}:{branch}&state=all
```

with a conditional request, so an unchanged answer costs a 304. If one
comes back, its snapshot becomes the item's subject, `base_ref` becomes the
pull request's base, and the item is a pull request item from then on, in
the same row, with the same threads and the same directory. For a project
whose roots are a fork, `{owner}` is the fork's owner and the pull request
is found on whichever of the project's repositories it was opened against;
the item's `repo_full_name` stays the fork.

The reverse also happens: opening a pull request from the inbox whose head
branch matches a checkout-only item in the same project attaches to that
item rather than making a second one.

### Attention

A notification resolves to an item by repository and number, using the
number the inbox already parses from the subject URL. When it resolves, the
item's `attention_at` is set to the notification's `updated_at` if newer,
and the notification's reason is kept as `attention_reason` so the Priority
row can say why: "review requested", "mentioned", "commented".

A notification with reason `review_requested` that resolves to no item
creates one: a pull request item with the snapshot fetched and no checkout.
This is the one case a notification makes an item, and it is the case the
tool exists for. Every other reason on an unknown subject stays in the
inbox.

A notification that resolves to an item is not shown in the inbox. It has
become the item's attention, and the item is where it is acted on. The
inbox shows the rest. The two sets are disjoint by construction, so there
is nothing to keep in step between them: Done and Saved on an item are the
item's, Done and Saved on an inbox notification are the notification's,
and no action on one touches the other. When a notification's subject is
opened from the inbox and becomes an item, the notification leaves the
inbox on the next paint.

Done on an item stores `attention_at` at the moment it was pressed, and
attention newer than that reopens the item, which is the rule the inbox
already uses. Saved is Perch's own flag and is not touched by activity.

CI is attention too. When a pull request snapshot's check status changes to
failed, `attention_at` is set to now and `attention_reason` to `ci_failed`.
Nothing else about the snapshot sets attention; a new commit on a pull
request you are reviewing arrives as a notification anyway.

### Archive

Archive is a state of the item, set by the menu or by the fortnight rule,
and described under the item menu. The directory goes only when Perch owns
it; the row, the branch, the threads and the knowledge stay. An item whose
subject is closed or merged is not archived by that fact; it drops out of
Active and is found under Closed. The fortnight rule archives items with
an owned checkout not opened in a fortnight, as today, and touches nothing
else: an issue or a root checkout is archived only by hand.

## Model

In `perch-github`, replacing `Worktree`:

```rust
pub struct Project {
    pub id: String,                       // the name, lower-cased
    pub name: String,
    pub repos: Vec<String>,               // owner/repo
    pub roots: Vec<PathBuf>,
    pub implicit: bool,                   // not in config
}

pub enum Subject {
    PullRequest(PullRequest),
    Issue(Issue),
}

pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,                    // open | closed
    pub state_reason: Option<String>,     // completed | not_planned
    pub author_login: String,
    pub author_avatar_url: String,
    pub labels: Vec<Label>,
    pub assignees: Vec<String>,
    pub comments: u32,
    pub updated_at: Timestamp,
    pub repo_full_name: String,
}

pub struct Checkout {
    pub branch: String,
    pub base_ref: String,
    pub head_sha: String,
    pub path: PathBuf,
    pub owned: bool,                      // under Perch's store
    pub on_disk: bool,                    // false while archived, or gone
}

pub struct Item {
    pub id: String,                       // ULID
    pub project_id: String,
    pub repo_full_name: String,
    pub subject: Option<Subject>,
    pub checkout: Option<Checkout>,
    pub from_item_id: Option<String>,     // the issue or item this was cut from
    pub name: Option<String>,             // from Rename
    pub saved: bool,
    pub pinned_at: Option<Timestamp>,
    pub done_at: Option<Timestamp>,       // attention_at when Done was pressed
    pub attention_at: Option<Timestamp>,
    pub attention_reason: Option<String>,
    pub archived_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub last_opened_at: Timestamp,
}

impl Item {
    pub fn title(&self) -> &str;          // name, else subject title, else branch
    pub fn needs_attention(&self) -> bool; // attention_at > done_at
    pub fn is_archived(&self) -> bool;
    pub fn is_closed(&self) -> bool;      // subject closed or merged, or done
    pub fn last_activity(&self) -> Timestamp;
}
```

An `Item` with neither subject nor checkout is invalid and the cache
refuses to insert one. Every other combination is a shape the UI knows how
to draw.

`PullRequest` gains `checks: Option<CheckStatus>` and
`review_decision: Option<ReviewDecision>` from the fields the pulls API
already returns with the snapshot, for the chips.

## Storage

```sql
CREATE TABLE IF NOT EXISTS projects (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    repos_json    TEXT NOT NULL,           -- ["owner/repo", ...]
    roots_json    TEXT NOT NULL,           -- ["/abs/path", ...]
    implicit      INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS items (
    id               TEXT PRIMARY KEY,
    project_id       TEXT NOT NULL REFERENCES projects(id),
    repo_full_name   TEXT NOT NULL,
    subject_kind     TEXT,                 -- 'pr' | 'issue' | NULL
    subject_number   INTEGER,
    subject_json     TEXT,                 -- the snapshot
    branch           TEXT,
    base_ref         TEXT,
    head_sha         TEXT,
    checkout_path    TEXT,
    owned            INTEGER NOT NULL DEFAULT 1,
    on_disk          INTEGER NOT NULL DEFAULT 1,
    from_item_id     TEXT REFERENCES items(id),
    name             TEXT,
    saved            INTEGER NOT NULL DEFAULT 0,
    pinned_at        TEXT,
    done_at          TEXT,
    attention_at     TEXT,
    attention_reason TEXT,
    archived_at      TEXT,
    created_at       TEXT NOT NULL,
    last_opened_at   TEXT NOT NULL,
    CHECK (subject_kind IS NOT NULL OR branch IS NOT NULL)
);
CREATE UNIQUE INDEX IF NOT EXISTS items_subject
    ON items(repo_full_name, subject_kind, subject_number)
    WHERE subject_kind IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS items_branch
    ON items(repo_full_name, branch)
    WHERE branch IS NOT NULL;
CREATE INDEX IF NOT EXISTS items_project ON items(project_id);
```

The two partial unique indexes are the attach rules in the schema: one
row per pull request or issue, one row per branch, and a row may be both.

`comment_threads` is created with `item_id` in place of `worktree_id`. The
`worktrees` table is gone. The `marks` table stays, keyed by notification
id, for the inbox, whose rows never resolve to an item.

There is no migration. `meta` carries a schema version, and a database
whose version is older than this milestone's, or that has no version, is
deleted and recreated on open, with a Notice saying so. The bare clones and
worktree directories under the store are left on disk; opening a pull
request finds the clone and re-adds the worktree, so the cost of the reset
is a fetch, not a clone.

Project rows are upserted from config on every startup: name, repos, and
roots follow the file, and a row that config no longer mentions but that
still has items becomes implicit rather than being deleted. A project with
no items and no config entry is deleted.

Cache methods:

- `load_projects() -> Vec<Project>`, `upsert_project(&Project)`,
  `delete_project(id)`.
- `load_items() -> Vec<Item>`, `insert_item(&Item)`, `update_item(&Item)`,
  `delete_item(id)`.
- `find_item_by_subject(repo, kind, number)`,
  `find_item_by_branch(repo, branch)`.
- `set_item_marks(id, saved, done_at)`.
- `touch_attention(id, at, reason)`, which only moves the time forward.
- `load_comment_threads(item_id)` and the rest, renamed from worktree.

## Backend

Commands replace the worktree set:

```rust
/// Open an item's view: review for a pull request or a branch, issue for
/// an issue. Prepares the checkout when there is one to prepare.
OpenItem { id: String },
/// Turn a notification into an item, or find the one it already has, and
/// open it. Replaces OpenPullRequest.
OpenSubject { repo_full_name: String, kind: SubjectKind, number: u64 },
/// Cut a branch from the default branch and make a checkout-only item
/// pointing back at the issue.
StartWork { issue_item_id: String },
/// Cut a branch from the item's head into a new checkout-only item.
ForkItem { id: String },
ArchiveItem { id: String },
RestoreItem { id: String },
DeleteItem { id: String },
MarkItem { id: String, saved: Option<bool>, done: Option<bool> },
PinItem { id: String, pinned: bool },
/// Attention now, reason `manual`.
MarkUnread { id: String },
RenameItem { id: String, name: Option<String> },
/// `project_id` of `None` with `new_project` set makes an implicit
/// project of that name first.
MoveItem { id: String, project_id: Option<String>, new_project: Option<String> },
/// Rescan roots and re-ask GitHub about subject-less branches.
RefreshItems,
```

`Ask` takes `item_id` in place of `worktree_id`. For an issue item the
prompt builder gets the issue instead of a pull request and no anchor, and
the run's cwd is the home checkout.

Events:

```rust
Projects(Vec<Project>),
Items(Vec<Item>),                         // whole, like Worktrees was
IssueReady { item: Arc<Item>, issue: Arc<Issue>, comments: Arc<Vec<IssueComment>> },
```

`ReviewReady` carries `item: Arc<Item>` in place of `worktree`, and
`pull_request` becomes `Option`.

Sequence on startup: load projects and items from the cache and emit both,
so the sidebar paints before the network. Then the inbox poll as today.
Then, off the main loop, scan roots and emit `Items` again if anything
changed. Then the head-branch lookups for subject-less items, one request
each, conditional, and emit `Items` if anything attached.

On each inbox poll, after threads are stored: resolve each thread to an
item, touch attention, create pull request items for review requests, and
emit `Items` once.

On `OpenItem` for an issue: fetch the issue and its comments, store the
snapshot on the item, archive the discussion under knowledge, emit
`IssueReady`, then `CommentThreads`. Offline, emit `IssueReady` from the
snapshot with the comments the cache has, and say so in the status line.

On `StartWork`: ensure the bare clone and the home checkout, fetch the
default branch, `git worktree add -b perch/issue-<n> <path> <default>`,
insert the item, emit `Items`, then proceed as `OpenItem` on it.

On `RefreshItems` and on the periodic scan: for each root, `git worktree
list --porcelain`; for each entry with a branch that is not the default,
find or insert an item with `owned = 0`; for each `owned = 0` item whose
path is missing, delete or archive per the rule above. Roots that are not
git checkouts, or that are checkouts of a repository outside the project,
are skipped with a Notice.

## Config

```toml
[[project]]
name = "perch"
repos = ["criccomini/perch"]
roots = ["~/Code/perch"]
```

`Config` gains `project: Vec<ProjectConfig>` at the top level, with
`roots: Vec<String>` added to `ProjectConfig`. `knowledge.project` is
removed; a config that still has it gets a Notice naming the new key.
`project_for` moves from `KnowledgeConfig` to `Config`.

## State

`AppState` loses `worktrees` and `screen: Screen { Threads, Worktrees }`
and gains:

```rust
pub projects: RwSignal<Vec<Project>>,
pub items: RwSignal<Vec<Item>>,
pub item_filters: RwSignal<ItemFilters>,
pub screen: RwSignal<Screen>,             // Sidebar | Inbox
pub open_item: RwSignal<Option<String>>,
```

```rust
pub struct ItemFilters {
    pub status: ItemStatus,               // Active | Closed | Archived | All
    pub kinds: ItemKinds,                 // three bools
    pub group: ItemGroup,                 // Project | Date
    pub sort: ItemSort,                   // LastActivity | LastOpened | Created
    pub show_empty: bool,
    pub show_pr_status: bool,
    pub query: String,
}
```

A pure function `sections(&[Project], &[Item], &ItemFilters) -> (Vec<Item>,
Vec<ItemSection>)` returns the Priority rows and the grouped rows, and is
tested the way the inbox's `Filters` is.

`ReviewState` gains `item: Arc<Item>` and `pull_request:
Option<Arc<PullRequest>>`. A new `IssueState` holds the issue, comments,
and the thread column's state, sharing the thread types with the review.

## UI

### Sidebar

The drawer's rail, segments, and the worktrees screen go. The drawer is
one scrolling column: header, bell, Priority, project sections. Project
sections collapse on click of the header and remember it in `meta`. The
selected row fills as inbox rows do. Rows take the same hover glyphs and
keys as inbox rows. The `...` on a row, and a right-click, open the item
menu: Open in, Pin, Mark unread, Rename, Fork, Move to project, Archive,
Delete, in that order with dividers after Open in, Fork, and Move to
project, each with its letter at the right edge, entries that do not apply
greyed. Rename swaps the title for a text box in place. Delete opens a
small confirmation under the row naming what goes.

### Filter menu

A popout under the filter button, rows as in the table above, each opening
a submenu on hover in the same style as the model picker. Toggles show a
check on the right.

### Inbox

The notifications screen as it is, opened by the bell and `Cmd-2`, with a
back glyph to the sidebar, over the threads that resolve to no item. The
filter that drops the rest is one pass over `items` before the existing
`Filters` run. Opening a pull request or issue from it goes through
`OpenSubject`, after which the row belongs to the sidebar.

### Issue view

Header: state icon, `#42`, title, then a row of label chips and assignee
avatars, then the Start work button on the right and the Done and Save
glyphs beside it. Body: the issue rendered from markdown, then a divider,
then each comment as an avatar, login, relative time, and body. Then a
divider labelled "In Perch" listing the items whose `from_item_id` is this
one, as sidebar-style rows. The thread column on the right, as in a
review.

### Review view

Unchanged for a pull request. For a checkout-only item the header shows
the branch, the base it is diffed against, and the directory, and omits
the pull request title, author, and counts. When the item gains a pull
request while open, the header fills in on the next `Items` event without
reopening.

## Harness

- `perch --items` prints the sidebar as text: Priority, then each project
  and its rows with their glyph letter, reference, and attention reason.
  It is how the sections function is checked without a window.
- `perch --scan` prints what the root scan found and what it would insert
  or remove, without writing.
- `perch --ask` takes an item id or `owner/repo#N` for an issue as well as
  a pull request.

## Build order

1. `perch-github`: `Project`, `Item`, `Subject`, `Issue`, `Checkout`; the
   two tables; the schema version and the reset; the cache methods; the
   config change. Tests: a database with no version or an old one is
   recreated on open; the unique indexes refuse a second row per subject
   and per branch; a row with neither is refused; `touch_attention` never
   moves backwards; `[[project]]` parses with and without roots. Commit.
2. `perch-app`: `ItemFilters` and `sections`, pure, tested: Priority holds
   only items with attention past Done; Active excludes closed, done and
   archived; Group by Date puts the project in the reference; search hits
   branch names. `perch --items`. Commit.
3. Backend: load and emit projects and items; `OpenItem` for pull request
   items over the existing prepare path; `OpenSubject`; `MarkItem`,
   `PinItem`, `MarkUnread`, `RenameItem`, `MoveItem`; `ArchiveItem`,
   `RestoreItem` and `DeleteItem` over the existing worktree code, with
   the item-level semantics. Notifications resolve to items on poll and
   set attention; review requests create items; the inbox drops threads
   that resolved. Commit.
4. UI: the new sidebar over the existing review view. Priority, Pinned,
   projects, rows, hover glyphs, keys, the item menu with every entry but
   Fork, inline Rename, the Delete confirmation, the filter menu, search.
   The inbox behind the bell. Remove the rail and the worktrees screen.
   Commit.
5. `perch-git`: home checkouts; the root scan; the branch cut shared by
   `StartWork` and `ForkItem`. `perch --scan`. Tests on a temporary
   repository with two worktrees. Commit.
6. Backend and UI: checkout-only items from Start work, Fork, and roots;
   the review view without a pull request; the head-branch lookup on
   refresh and the attach in both directions. Commit.
7. `perch-github` and knowledge: fetch an issue and its comments; archive
   under `issues/`; extend the skill text. Commit.
8. Backend and UI: the issue view, `IssueReady`, `Ask` on an issue in the
   home checkout, Start work from the header, the "In Perch" list. Commit.
9. README: rewrite the opening and the worktrees section around projects
   and items; the config keys; the keys table. Commit.

Steps 1 and 2 are independent. Steps 5 and 7 are independent of 4.

## Acceptance

- Launch over a `perch.db` from the previous milestone. The status line
  says the database was reset. The sidebar is empty but for what config
  declares. Open a pull request that had a worktree before: it is ready
  after a fetch, not a clone, and its directory is the one it had.
- A review request arrives. The pull request appears in Priority with
  "review requested" and under its project, before it is opened, and does
  not appear in the inbox. Open it, press `e`. It leaves Priority. Someone
  comments; it comes back.
- A comment on a pull request you have never opened, in a project you
  declared, shows in the inbox. Open it from there: it becomes a row under
  the project and leaves the inbox. The bell's count drops by one.
- Declare a project with a root that has a branch checked out and one git
  worktree. Both appear as branch rows with `+n −m`. Open one: the diff is
  against the default branch, the header names your directory, threads
  work. The `...` menu has no Archive.
- Push that branch and open a pull request on github.com. Within a
  refresh the same row shows the PR number and state, the header fills in,
  the threads are still there.
- Open an issue from the inbox. The issue view shows body and comments.
  Ask "where would this change go"; the run lists files read from the home
  checkout and answers. Press Start work: a branch row appears under the
  project, the review opens with an empty diff, the issue view lists the
  branch under "In Perch". `~/.perch/knowledge/.../issues/<n>.md` exists.
- Set Status to Closed. Merged pull requests and Done items appear;
  nothing else. Set Group by Date; sections are dates and rows name their
  project. Type a branch name in search; only that row remains. Quit and
  reopen; the settings held.
- On a pull request row: Pin, and it appears under Pinned and not under
  its project. Rename it; the row shows the name, the reference line still
  shows the number, and search finds either. Mark unread; it appears in
  Priority with "manual", and `e` clears it. Fork; a branch row appears
  with an empty diff against the source's head. Move it to a new project
  typed in the submenu; the section appears with one row. Archive; it is
  gone from Active, found under Archived, and its directory is gone.
  Restore; it is back and opens after a fetch. Delete; the confirmation
  names the directory and the branch, and after it both are gone along
  with the threads.
- On a root checkout row: Open in shows Finder, Terminal, Copy path and
  GitHub greyed. Archive hides it and the directory is untouched. Delete
  removes the row, the directory is untouched, and the next refresh lists
  it again with no threads.
- Remove a root's worktree on disk. On the next refresh its row is gone if
  it had no threads, or archived and greyed with no Restore if it had.
- A mention in a repository no project claims shows in the inbox behind the
  bell and creates neither a project nor an item.
- `perch --items` and `perch --scan` print what the window shows.
- `cargo test` green, `cargo clippy` clean.

## Things the implementer should not do

- Do not add a `kind` column to `items`. The shape is what the row has.
  If a function needs to branch on it, derive it from the optionals.
- Do not turn an issue item into a branch item. Start work makes a new
  row with `from_item_id`. The issue keeps its row.
- Do not touch a directory Perch did not make. Archive and Delete on a
  root checkout act on the row. The `owned` flag is the gate.
- Do not hide menu entries. Grey them. The menu is the same shape on every
  row, and a greyed entry says why on hover in one line.
- Do not let the single-letter menu keys work on a bare row. `a` is taken
  by the review view and `e` and `s` by the inbox; the letters live inside
  the open menu.
- Do not create an item for a notification unless the reason is
  `review_requested`. Everything else waits for a click.
- Do not create a project from a notification. Implicit projects come from
  items, and items come from opening things or from roots.
- Do not list branches. Worktrees and the checked-out branch of a root are
  items; `refs/heads` is not.
- Do not fetch anything for a row. Chips come from the snapshot. The
  sidebar is paintable from the cache alone.
- Do not make the home checkout an item, and do not let the fortnight rule
  see it.
- Do not write a migration. The schema version check deletes and
  recreates. If a later change needs the data kept, that milestone writes
  the migration from this schema, not from the one before it.
- Do not sync marks between items and notifications. A notification that
  resolves to an item is not in the inbox, so there is nothing to sync. If
  a sync seems needed, the inbox filter is wrong.
- Do not change the notifications screen beyond how it is reached and
  which threads it is given. It is the fallback for everything that is not
  an item and it works.
- Do not block the first paint on the root scan or the head-branch
  lookups. Cache, paint, then the network.
- Do not rewrite the review view. It takes an `Option<PullRequest>` and
  draws less when it is `None`.
