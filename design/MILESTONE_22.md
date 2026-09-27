# Perch: Milestone 22, Inbox, Worktrees and Browse

## Goal

The sidebar's three scopes are All, Notifications and Authored. All and
Authored are views over Perch's local items, and an item is made as a
side effect of opening something: view a pull request from a
notification and it has a row for good. Rows leave by a mix of rules
(the Status filter, Archive, a 14-day sweep that runs once at launch,
the root scan) that nobody can predict from the sidebar. There's no way
to open an arbitrary pull request or issue from the UI at all, only the
`PERCH_OPEN_*` launch variables. And 44 GB of worktrees sit under
`~/.perch/worktrees`, beside a 17 MB database.

After this milestone the sidebar answers three questions, one tab each:

- **Inbox**: what GitHub wants me to look at. The notifications, as
  now.
- **Worktrees**: what's on my disk. One row per worktree, and a row
  lasts exactly as long as its directory.
- **Browse**: what else is on GitHub. A few fixed views (Review
  requested, Yours, Assigned, Mentioned, By repository) and **Recent**,
  the things you opened lately.

**Open…** in the Perch menu, `Cmd-O`, opens any pull request or issue
from a URL or a reference. And Perch's own worktrees clean themselves up
in the background, by one rule, with no button and no prompt.

## Non-goals

- Other platforms. Browse is written against GitHub's API directly. No
  provider interface.
- Custom search queries. The views are fixed.
- Stopping opens from making items. Threads, runs and reviews are keyed
  by item id, and moving them is its own project. Here, an item without
  a worktree is simply listed in Recent rather than in a permanent list.
- Removing, or ever touching, a worktree Perch didn't make. Worktrees
  the root scan finds under your own clones come and go as you make and
  remove them.
- Disk sizes, anywhere.
- A Clean up button, "safe to remove" marks, or any other manual
  cleanup surface.
- Pruning `review-snapshots` (1.6 GB here). Findings may point into
  them; that needs its own look.
- `perch open <url>` and a `perch://` handler. Follow-up.
- An open/closed filter on Inbox. GitHub's notification list doesn't
  say whether the subject is open, and a lookup per thread isn't worth
  it.

## Behaviour

### The tabs

`scope_tabs` becomes **Inbox · Worktrees · Browse**, in that order.
Inbox is the default. The tab, and each tab's filters, are kept in
`meta` as the filters are now; search stays transient.

Save folds into Pin. A row's trailing end shows the pin, the unread dot
and the age, and on hover **Pin** and the menu, where it showed Save.
The bookmark glyph goes. Every saved item becomes pinned when the
database migrates (see [Model](#model)).

The search box above the list searches the tab it's on:

- Inbox: as the Notifications scope's search does now.
- Worktrees: as All's does now, over title, given title, number, branch
  and `owner/repo`.
- Browse: the loaded results of the current view, over the same fields,
  with no request to GitHub. When the text is a reference Open…
  accepts (see [Open…](#open)), the first row is **Open
  slatedb/slatedb#57**, and Enter opens it.

### Inbox

The Notifications scope, unchanged, with one filter added at the top of
its menu:

- **Show**: Unread · All. Default All, which is what the list shows
  today.

Repository and Reason stay as they are.

### Worktrees

Every item with a checkout on disk: Perch's own under
`~/.perch/worktrees`, and those the root scan finds under a `roots`
entry. Issues never appear; they have no checkout.

- Pinned rows first, most recently pinned first, then the rest by the
  sort. The filter menu keeps Repository, Sort By (Last activity, Last
  opened, Created), the direction and Show PR Status. Kind, Status and
  Saved only go.
- A row for a pull request whose pull request has merged or closed
  shows that state, as it does now. It stays until its worktree goes.
- The row menu: Pin, Mark Unread, Rename, Fork, **Remove Worktree**,
  Delete. Remove Worktree is today's Archive, renamed, on the same
  key. It refuses on uncommitted changes and asks about commits that
  exist nowhere else, as now. It no longer asks about ignored files
  (see [Cleanup](#cleanup)).
- When a worktree goes, by Remove Worktree, the sweep or the root scan,
  its row leaves the tab. The item itself stays, with its threads and
  runs, and is listed in Recent.

### Browse

A view chip at the top of the list, and an **Open · Closed** toggle
beside it, default Open. Closed includes merged.

| View              | GitHub search                              |
| ----------------- | ------------------------------------------ |
| Review requested  | `is:pr review-requested:@me`               |
| Yours             | `author:@me`                               |
| Assigned          | `assignee:@me`                             |
| Mentioned         | `mentions:@me`                             |
| By repository     | `repo:<owner/repo>`                        |
| Recent            | local, see [Recent](#recent)               |

Each GitHub view adds `is:open` or `is:closed` from the toggle, and
`archived:false`, and sorts by `updated-desc`. By repository has a
second chip listing the configured GitHub repositories
(`repository_options` without `local/` ones), opening on the last one
picked.

Rows show the kind glyph, the title, `owner/repo #57`, the author and
how long since the last update. A draft, merged or closed one says so.
A row whose inbox thread is unread is bold with the dot
(`subject_unread`). Clicking a row opens it through `open_subject`, as
the "Closes #N" chip does.

Fetching:

- A view is fetched when it's selected, and again after each
  notification poll while it's showing. A refetch that would add,
  remove or reorder rows is held back behind the news pill, as the
  Inbox's is.
- 50 results a page. **Show more** at the foot loads the next page.
- Results live in memory only, per view and toggle, until Perch quits.
  Nothing goes in the database.
- No token: the same message the Inbox shows. A failed search keeps the
  last results and says what failed over them. Offline says `Offline`.

GitHub's search API allows 30 requests a minute. A view refetches at
most once per poll, so switching views is the only way to spend them
quickly, and a `403` for the search limit waits for the reset GitHub
names rather than retrying.

### Recent

Every item without a worktree on disk, most recently opened first: pull
requests and issues you opened, and branches whose worktree went. Pinned
rows first, and they never drop off. Then the 50 most recently opened.
Older ones aren't shown, and aren't deleted; opening one again brings it
back to the top.

Recent has no Open/Closed toggle and no filter. Each row shows its
state.

The row menu: Pin, Mark Unread, Rename, Start Work on an issue, Restore
Worktree on a branch whose worktree Perch made (today's Restore), and
Delete. A branch the root scan found, whose directory has gone, opens
as an archived row opens today and has no Restore.

### Open…

**Open…** goes in the Perch menu under New Window, with `Cmd-O`. It
has a target of its own as New Window does, so it works with no window
open: it opens one first.

It shows a one-line box over the window. It takes:

- A URL: `https://<host>/<owner>/<repo>/pull/<n>` or `/issues/<n>`, for
  `github.com` and each host in `github_hosts()`. Anything after the
  number (`/files`, `#issuecomment-…`) is ignored.
- A reference: `owner/repo#57`.
- A bare `#57` or `57`, in the repository of the open item, else the
  one By repository has picked, else the only configured GitHub
  repository. With none of those it says `Which repository?`.

A URL says whether it's a pull request or an issue. A reference doesn't,
so Perch asks `GET /repos/{repo}/issues/{n}`, whose `pull_request` key
answers it, and then opens through `open_subject`. A number that
doesn't exist says `No #57 in slatedb/slatedb`, in the box. Enter opens,
Escape closes.

Whatever is opened goes into Recent, as anything opened does.

### Cleanup

The sweep removes Perch's own worktrees. It never touches one the root
scan found.

A Perch-owned worktree is **removable** when it has:

- no modified or untracked files (`is_dirty`),
- no commits that exist nowhere else (`commits_only_on`).

Ignored files don't count. `git worktree remove` treats them the same
way, and so does Remove Worktree now.

A removable worktree is **removed** when either:

- its pull request has merged or closed, or
- it hasn't been opened in 14 days (`last_opened_at`, which is set when
  the item is made).

This is one rule for pull request checkouts and branches of your own.
Today's sweep never removes a branch; it now does, because the branch
and its commits stay in the repository and unpushed commits block
removal, so nothing can be lost that git doesn't hold.

The sweep skips a worktree that is open in any window, or whose item
has an agent run working.

It runs at launch, as now, and after each notification poll. Before
judging "merged or closed", it refreshes the pull request of each
Perch-owned checkout, conditionally, as `attach_pull_requests` does, so
a merge nobody opened in Perch is still seen. Each removal goes through
`archive_item` under the item lifecycle lock, as today. A refusal (the
worktree turned dirty since) leaves it, and the next sweep looks again.

## Model

- `ItemScope` becomes `SidebarTab { Inbox, Worktrees, Browse }`.
- `Item::saved` goes. A preserving migration sets `pinned_at` to
  `last_opened_at` on every item with `saved = 1` and no `pinned_at`,
  so saved rows sort among the pins by when they were last opened. The
  `saved` column stays, unread, and is written as 0; dropping it isn't
  worth a table rebuild.
- `ItemFilters` loses `status`, `kinds` and `saved_only`, and gains
  `inbox_unread_only: bool`, `browse: BrowseView` and
  `browse_closed: bool`. `BrowseView { ReviewRequested, Yours,
  Assigned, Mentioned, Repository(String), Recent }`.
- `ItemFilters::parse` takes the old keys: `scope=all` is Worktrees,
  `notifications` Inbox, `authored` Browse with Yours. `status`, `kinds`
  and `saved` are checked and ignored, as `group` is.
- `ItemStatus` goes. `visible_items` splits into `worktree_items` and
  `recent_items`. `Item::is_archived` stays; the column still records
  that a worktree went, and is how Recent finds branches.
- `AppState` gains `browse_results: RwSignal<HashMap<(BrowseView,
  bool), BrowsePage>>` and the Open box's text and error.

## Backend

- `perch-github`: `search_issues(query, page, per_page) ->
  Vec<SearchHit>`, `SearchHit { kind, repo_full_name, number, title,
  state, draft, merged, author_login, author_avatar_url, updated_at,
  html_url }`. `merged` is `pull_request.merged_at` being set.
  `subject_kind(repo, number)` from the issues endpoint.
- The commands `Browse { view, closed, page }` and `OpenReference {
  repo_full_name, number, kind: Option<ItemKind> }`, with the events
  `BrowseResults` and `OpenReferenceFailed`.
- Reference parsing is a pure function in `perch-app`, beside the
  launch hooks' `owner/repo#N` parsing, which moves onto it.
- `sweep_items`: drop the branch exemption, add the merged/closed
  trigger and the refresh before it, skip open and working items, and
  run after each poll.
- `archive_item`: drop the ignored-files `ConfirmLoss`. Delete's
  commits prompt stays.

## Harness

Headless tests in the style of the ones they sit beside.

- Tabs: Inbox is the default; each tab's filters round-trip through
  `meta`; the old `scope`, `status`, `kinds` and `saved` keys parse.
- Save into Pin: the migration pins a saved item and leaves an already
  pinned one's `pinned_at` alone; a row offers Pin on hover and no
  Save.
- Worktrees: lists owned and scanned checkouts on disk and nothing
  else; a removed worktree's item moves to Recent with its threads.
- Recent: items without a worktree, by `last_opened_at`, capped at 50
  with pins on top and exempt; opening an old one brings it back.
- Browse: each view builds its query with `is:open` or `is:closed`;
  results parse, including draft and merged; a refetch that reorders
  rows is held back; Show more asks for the next page; a search `403`
  waits.
- Open…: URLs for `github.com` and a configured host, with and without
  trailing paths; `owner/repo#N`; bare `#N` against the open item, the
  picked repository, the one configured repository, and none. A
  reference asks for its kind; a URL doesn't.
- Cleanup: removes a clean pull request checkout whose pull request
  merged; removes one unopened 15 days; keeps one unopened 13 days;
  removes a clean branch unopened 15 days and keeps its branch; keeps a
  branch with unpushed commits, a dirty one, one with untracked files,
  one open in a window, one with a working run, and every scanned
  worktree. Ignored files don't block removal. A new worktree isn't
  removed.

## Build order

1. Open…: parsing, `subject_kind`, `OpenReference`, the box, the Perch
   menu item. The launch hooks use the parser. Commit.
2. The tabs: `SidebarTab`, Worktrees and Recent over today's items,
   the filter changes and `meta` migration, Remove Worktree, Unread/All
   on Inbox, Save folded into Pin with its migration. Browse shows
   Recent only for now. Commit.
3. Cleanup: the new sweep rule, the refresh, the poll hook, and the
   ignored-files prompt gone. Commit.
4. Browse's GitHub views: `search_issues`, the chips, the toggle,
   fetching, the held-back pill, Show more, the search box's Open row.
   Commit.
5. README: **Data** gains the cleanup rule in a few lines: Perch
   removes its own worktrees under `~/.perch` once they're clean and
   their pull request has closed or they've gone 14 days unopened,
   keeps the branch, never touches a worktree under your `roots`, and
   treats ignored files as `git worktree remove` does. Commit.

## Acceptance

- Launch: the sidebar opens on Inbox. Worktrees lists this repository's
  root checkouts and Perch's own. Browse › Recent lists the pull
  requests and issues opened before, most recent first.
- Press `Cmd-O`, paste a pull request URL from github.com: it opens,
  and it's at the top of Recent. Type `slatedb/slatedb#<an issue>`: it
  opens as an issue. With a slatedb item open, type `#<n>`: it opens in
  slatedb. Close every window, press `Cmd-O` from the menu: a window
  opens with the box.
- Browse › Review requested lists what GitHub's review-requested page
  lists. Flip to Closed: merged ones say merged. Pick By repository ›
  slatedb/slatedb: its open pull requests and issues. Paste a URL into
  the search box: the Open row is first.
- Start work, let it make a worktree, and don't commit. Wait for two
  polls: it's still there. Merge a pull request of yours on github.com
  whose Perch checkout is clean: within a poll or two its row leaves
  Worktrees and appears in Recent, the directory is gone, and the branch
  is still in the clone. Put a `.env` in another merged one: it goes
  too. Leave a file modified in a third: it stays.
- Set a clean branch's `last_opened_at` back 15 days in the database:
  the next poll removes its worktree and `git branch` still lists it.
  Restore Worktree from Recent brings it back.
- A worktree under `~/Code` whose pull request merged stays, however
  old.

## Things the implementer should not do

- Don't remove, move or write to a worktree the root scan found.
- Don't delete an item because its worktree went. Its threads, runs and
  reviews stay.
- Don't remove a worktree with modified or untracked files, or with
  commits that exist nowhere else, and don't commit, stash or push to
  make one removable.
- Don't store Browse results in the database.
- Don't add a provider abstraction or a query box.
- Don't run `cargo fmt --all`. Format only new files.
