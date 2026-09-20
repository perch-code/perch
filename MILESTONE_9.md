# Perch: Milestone 9, The repository is the unit

> Historical specification. The repository-history crawl and knowledge
> archive described here have been removed; see [README.md](README.md).

## Goal

Make the repository the unit of everything: what is declared, what is
listed, what is synced, what is grouped, and where knowledge lands.
Then make every open pull request and issue in a declared repository
an item, without anyone having to be notified about it, and fill
knowledge with the whole of a repository's discussion, open and closed.

Today there is a project between the reader and the repository. A
project has a name, a list of repositories and a list of roots; an item
belongs to a project; knowledge is filed under the project; a repository
nobody declared gets an implicit project named after it; and declaring
it later adopts the items. All of that exists so two repositories can
share a sidebar heading. The knowledge skill does not need it: it is one
file over the whole store, and an agent finds its way by matching `git
remote` against an index. So the project is a grouping label carried as
an entity, with a table, an id, an implicit flag, a Move to project
menu, and a `project_for` lookup that every path runs through.

After this milestone a repository is declared as `[[repo]]`, an item
belongs to a repository, the sidebar groups by repository or by owner,
knowledge is filed by `owner/repo`, and the word project is gone from
the code.

Then the sync. Today an item comes from one of three places: a
checkout found under a root, a notification the reader opened, or a
review request, which is the one notification that makes an item on
its own. Everything else a repository is doing is in the inbox or
nowhere. The sidebar for slatedb shows three items; github.com shows
twenty-odd open pull requests. Knowledge holds the discussion of
whatever was opened in Perch, and nothing else.

After this milestone the backend lists the open pull requests and issues
of every declared repository on each poll and keeps an item for each.
A pull request that was open last time and is not in the list now is
fetched once more to learn whether it merged or closed, and its row says
so. A notification no longer decides what exists; it says why something
existing needs a look. And a background task walks each repository's
history, oldest first, and archives every discussion under knowledge,
so an agent asked why something was done can find the answer whether or
not the reader ever opened it.

This is the approach Forge takes, and the reason it has no inbox: when
the open set is already on screen, a notification has nothing to add
but a reason. Milestone 10 removes the inbox on that basis. This one
makes it removable.

## Non-goals

- Removing the inbox, the bell, or the thread marks. Milestone 10.
- A named group of repositories. Group by Owner is the way to see
  `slatedb/slatedb` and `slatedb/slatedb-go` under one heading. If a
  group with a name of the reader's own turns out to be wanted, it is a
  `group` field on `[[repo]]` and a fourth grouping, and it waits until
  someone wants it.
- Keeping the old database. The `projects` table and `items.project_id`
  go, the schema version bumps, and the database is recreated. Nobody
  is running Perch against a database they need. Milestone 8 said the
  next schema change would write a migration; it does not, and
  Milestone 10 says the same.
- Closed history in the sidebar. Closed pull requests and issues the
  reader never touched go to knowledge and not to `items`. The list is
  sent whole to the window on every change, and a repository's whole
  history in it would be thousands of rows serving a status filter.
- Repositories that are not declared. An item in an undeclared
  repository, made by opening a notification or by a checkout under a
  root, is refreshed on open as today. The list pass runs over the
  declared repositories and nothing else.
- Workflow runs as items. A CI notification still refreshes the checks
  of the open pull request and does nothing else. A row for a failed
  run on the default branch is a later milestone, if at all.
- A hot and warm refresh cadence per item. Every open item is refreshed
  by the list pass on every poll, because the list is one request and
  its fields are what the sidebar shows. The discussion behind an item
  is refreshed on open, on a notification, and by the backfill when the
  item's `updated_at` has passed the discussion's `fetched_at`. That is
  three triggers and no scheduler.
- GraphQL, a request budget, GitHub Apps, or a reserve. One token, a
  handful of repositories, conditional requests. The backfill stops
  when the remaining allowance is low and that is the whole policy.
- Writing to GitHub.

## Behaviour

### The repository is the project

Config:

```toml
[[repo]]
name = "slatedb/slatedb"
roots = ["~/Code/slatedb"]     # optional; checkouts you already have

[[repo]]
name = "slatedb/slatedb-go"
```

`name` is `owner/repo` and is the identity, compared case-insensitively.
`roots` is what `project.roots` was, per repository. A directory that
holds worktrees of two repositories is listed under both. `[[project]]`
is a config error that names the new key.

An item belongs to a repository by `repo_full_name`, which it already
carries. `project_id` goes. An item in a repository nobody declared is
still an item, and its repository appears in the sidebar as a section
the same as a declared one, with no badge and no distinction, because
there is nothing to distinguish: it is a repository with items in it.
Declaring it later starts the list pass and the backfill for it and
changes nothing about the rows that were there.

The sidebar groups by **Repository**, by **Owner**, or by **Date**.
Repository is the default and is one section per repository with items,
declared ones first in config order, then the rest by name. Owner is one
section per `owner`, repositories inside it in the same order, and a
row's second line shows the repository since the heading no longer
does. Date is what it is. Show Empty Projects becomes Show Empty
Repositories and lists declared repositories with nothing to show.

The row's second line under Repository is `#57 · branch`, since the
heading says the repository. Under Owner and Date it is `owner/repo #57
· branch`, as today.

The row menu loses Move to project and New project. An item is in the
repository its subject or its checkout is in, and that is not a choice.

Knowledge loses the project level. The layout is:

```
~/.perch/knowledge/
  index.md                          repositories, last updated
  slatedb/slatedb/
    index.md                        this repository's containers and how current
    raw/github.jsonl                one record per line; the truth
    raw/perch.jsonl                 your own threads, the same way
    github/pulls/812.md             rendered from raw
    github/issues/42.md
    perch/pulls/812.md              what you asked an agent about 812
```

`Message.project` becomes `Message.repo`, `owner/repo`, and the
container keeps its `owner/repo#812` form so ids do not change. The
root index maps each repository to its directory and the skill's text
says to match `git remote get-url origin` against `owner/repo`, which is
what it says now with one fewer step. An existing knowledge directory in
the old layout is left alone and a fresh one is written beside it; the
README says to delete the old one.

### The list pass

On every poll, before the notification poll, for each declared
repository in config order:

1. `GET /repos/{owner}/{repo}/pulls?state=open`, paged, with
   `If-None-Match` set to the ETag stored from the last pass. A 304
   means no pull request opened, closed, or changed, and the repository
   is skipped for pull requests. A 200 replaces the ETag and the pass
   continues.
2. Each pull request in the list is upserted as an item: found by
   `(repo, PullRequest, number)`, or by a checkout of the reader's own
   on its head branch, which takes the subject the way a review request
   makes it take one today; else made new.
   The stored subject is replaced only when the incoming `updated_at`
   is not older than the stored one, so a detail fetch that landed
   between two list pages is not overwritten by the earlier page.
3. Every item in the repository whose subject is open in the cache and
   whose number is not in the list is fetched by number. Its subject is
   stored as GitHub reports it: merged, closed, or, rarely, still open
   and missed by a page boundary. A closure enqueues one last archive of
   its discussion.
4. The same for issues, with `GET /repos/{owner}/{repo}/issues?state=open`,
   dropping the entries that are pull requests, which that endpoint
   also returns.

The ETag is written only after every upsert and closure for that
endpoint succeeded. A pass that failed partway leaves the old ETag in
place, so the next pass asks unconditionally and applies what the failed
one did not. This is the one rule that keeps a partial failure from
freezing the cache until something else changes upstream.

An item the pass makes has no checkout, `created_at` and `last_opened_at`
of now, and no attention. It is a row under its repository, sorted by
its subject's `updated_at`, and nothing marks it as new. Priority is
still about attention.

The pass runs inline in the poll loop before `list_threads`, so a
notification about a pull request the pass just made finds the item.

### Attention

`attend` no longer makes items. Every branch that did is gone and the
review-request case with it, because the list pass has already made the
item. What remains is the attention rule, and it narrows:

| Reason | Effect |
|---|---|
| `ci_activity` | refresh the open pull request's checks, as today; attention with reason `ci_failed` when the checks fail |
| everything else: `review_requested`, `mention`, `team_mention`, `assign`, `comment`, `author`, `subscribed`, `state_change`, ... | attention, with that reason |

Every reason github.com's inbox lists makes attention, so Needs
attention in the sidebar is the inbox on the web, row for row. (An
earlier draft left `subscribed` and `state_change` out, to keep every
push on every watched pull request out of Priority; Priority is gone,
and a reader who finds subscribed activity noisy leaves it out with the
Reason filter instead.)

A notification about a branch row that gains its pull request later in
the same poll is applied to the row then, from the poll's list, rather
than a poll later.

An item in an undeclared repository still gets attention from its
notification, as today, when the item exists. A notification about a
subject with no item does nothing; the inbox still shows it until
Milestone 10.

### Closed items

An item whose subject closed stays. It is `Closed` under the status
filter, keeps its threads, its checkout, and its knowledge, and can be
archived or deleted as today. Nothing here removes rows.

### The backfill

A task started once the first list pass of the session has finished,
running beside the poll loop at the pace the rate limit allows:

1. **Open items first.** Every item with a subject whose `discussions`
   row is missing, or whose `fetched_at` is older than the subject's
   `updated_at`, gets its discussion fetched, stored, and ingested into
   knowledge. Pull requests through `fetch_discussion`; issues through
   `fetch_issue_comments`. Newest `updated_at` first, so what is moving
   is archived before what is not.
2. **Then history.** For each declared repository, closed pull requests
   by `?state=closed&sort=updated&direction=asc`, then closed issues
   the same way, one page at a time, with the page number kept in
   `meta` so a restart resumes where it stopped. Each is fetched and
   ingested into knowledge under the repository, and not stored as an
   item. When the last page is reached the cursor records
   the `updated_at` of the last entry, and later runs page from there
   with `since` for issues and by skipping older entries for pull
   requests, which have no `since`.
3. **Then it sleeps** until the next poll and starts at step 1 again.

Between fetches the task checks the allowance the client last saw in
`x-ratelimit-remaining`. Below 1,000 it waits until the reset the
client last saw, then continues. The list pass and anything the reader
does are never held by the backfill; they run on the same client and
take their requests as they come.

The status bar shows the backfill while it runs, after the poll status:
"Archiving slatedb/slatedb, 312 of about 2,140", where the total is the
open count plus what the first closed page's `Link` header implies. When
every repository is caught up the line is absent.

### Knowledge

Nothing changes in what a discussion becomes. `to_messages` and the
issue equivalent are what they are, `ingest` is idempotent by message
id, and the raw file and the rendered markdown land where they land
today. What changes is coverage: after the backfill has finished once,
`knowledge/<owner>/<repo>/github/pulls/` holds every pull request the
repository has had, and `issues/` every issue.

The skill text says so. "Discussion history for slatedb from GitHub pull
requests and issues" becomes accurate rather than aspirational, and the
sentence about coverage says that closed history is complete once the
backfill has run and names the command that shows whether it has.

## Model

`perch-github`, the repository:

- `RepoConfig { name: String, roots: Vec<String> }` replaces
  `ProjectConfig`; `Config.repo: Vec<RepoConfig>` replaces
  `Config.project`. `Config::is_declared(repo_full_name) -> bool` and
  `Config::roots_for(repo_full_name) -> Vec<PathBuf>` replace
  `project_for`. `KnowledgeConfig.project` goes.
- `Project` goes, and `Cache::upsert_project`, `load_projects`, and the
  `projects` table with it. `Item.project_id` goes. `Cache::repos() ->
  Vec<String>` lists the distinct `repo_full_name` of every item, which
  with the config's list is what the sidebar sections over.

`perch-knowledge`:

- `Message.project` becomes `Message.repo`. `RawKey` is `(repo,
  source)`; the group was the repository and is now redundant. The
  render paths, the two indexes and `rendered_relative` follow the
  layout above. `SkillConfig.projects` becomes `repos`.

`perch-app`:

- `ItemGroup` is `Repository`, `Owner`, `Date`, keys `repo`, `owner`,
  `date`. `sections` takes the declared list and the items and no
  projects. `ItemSection.count` is the repository's active items under
  Repository and Owner. `reference` takes the group.
- `Command::MoveItem`, `new_project_for`, `new_project_box`, the Move to
  project submenu, `sync_projects`, `drop_empty_implicit`, and
  `Ctx::project_for` go. `scan_roots` walks `config.repo`.
- `perch --items` keys: `group=repo|owner|date`; `show_empty` is
  unchanged.

`perch-github`, the sync:

- `Client::list_open_pulls(repo, etag: Option<&str>) -> Listed<PullRequest>`
  and `list_open_issues(repo, etag) -> Listed<Issue>`, where

  ```rust
  pub enum Listed<T> {
      Unchanged,
      Changed { etag: Option<String>, items: Vec<T> },
  }
  ```

  Paging is `list_all`'s, with the conditional header on the first page
  only; a later page is never conditional. The issues variant drops
  entries with a `pull_request` key.
- `Client::list_closed_pulls(repo, page) -> Page<PullRequest>` and
  `list_closed_issues(repo, page, since: Option<Timestamp>) -> Page<Issue>`,
  where `Page { items, last_page: Option<u32> }` and `last_page` comes
  from the `Link` header when present.
- `Client::allowance() -> Option<Allowance { remaining: i64, reset_epoch: i64 }>`,
  the last `x-ratelimit-*` pair any request saw, kept in the client
  behind an atomic. Every request path already reads the headers to
  classify errors; they update the allowance on success too.
- `Cache::open_subjects_in(repo) -> Vec<(ItemKind, u64, String)>`, the
  kind, number, and item id of every item in a repository whose subject
  is open.
- `Cache::discussion_stale() -> Vec<String>`, item ids whose discussion
  row is missing or older than the subject's `updated_at`, newest
  subject first.

`perch-app`:

- `Reason::makes_attention(&self) -> bool`, the table above, in
  `perch-github` beside `is_participating`. `attend` uses it.
- `backend::sync` module: `list_pass(ctx, repo) -> Result<PassReport, Error>`
  where `PassReport { added, updated, closed }`, and `backfill(ctx)`,
  the task. Both are functions over `Ctx` so the harness can run them
  without the window.

## Storage

`projects` is dropped and `items` loses `project_id` and its index. The
schema version bumps to 11 and the database is recreated. Items,
threads with agents, marks, viewed sets and discussions are lost on
first launch; the list pass and the backfill rebuild the items and the
discussions within a poll, and knowledge on disk is untouched. Say so in
the commit message.

`meta` gains, per repository:

- `etag:pulls:<owner/repo>`, `etag:issues:<owner/repo>`.
- `backfill:pulls:<owner/repo>`, `backfill:issues:<owner/repo>`, each a
  page number while paging and `done:<updated_at>` after.

## Backend

- The poll loop calls `sync::list_pass` for each declared repository
  before `refresh`. A pass that fails logs and moves to the next
  repository; the poll status reports the first failure the way it
  reports a failed `list_threads` today.
- `attend` loses item creation and gains `makes_attention`.
- `open_item` for a closed subject is unchanged: the review view shows a
  merged pull request as it does now, from the checkout if there is one.
- A closure enqueues the item id for the backfill's step 1 rather than
  fetching inline, so the list pass stays two requests per repository.
- `send_items` is called once per list pass, not once per upsert.
- The backfill is a `tokio::spawn` from the loop after the first pass,
  holding a clone of `Ctx`, sending `Event::Backfill { repo, done,
  total }` as it goes and `Event::Backfill` with `total: 0` when
  finished. It is cancelled with the loop.

## Harness

- `perch --sync <owner/repo>` runs one list pass for the repository
  against the real cache and prints the report: each item added, updated
  or closed, one line each, and "unchanged" on a 304.
- `perch --backfill <owner/repo> [--limit <n>]` runs the backfill for one
  repository in the foreground, printing each discussion archived and
  the allowance after it, stopping after `n` when given. Without a
  limit it runs to the end.
- `perch --items` is unchanged and shows the result.

## Build order

1. `perch-github`: `RepoConfig` and the config parse, with `[[project]]`
   refused by name. `Project` and its table gone, `project_id` gone,
   `Cache::repos`, the schema bump. Tests: the config round-trips; an
   old key is an error that names the new one. Commit.
2. `perch-knowledge`: `repo` on `Message`, the layout, the indexes, the
   skill text. Tests: a message renders to the new path; the root index
   lists repositories; `rerender` over the new layout. Commit.
3. `perch-app`: `ItemGroup` over repository and owner, `sections` with
   no projects, `reference`, the row menu without Move, `scan_roots`
   over repos, `perch --items`. Tests: declared repositories lead in
   config order; an undeclared one follows by name; Owner nests; the
   second line drops the repository under Repository only. Commit.
4. README: the config section, the knowledge layout, the row menu, and
   the note to delete an old knowledge directory. Commit.
5. `perch-github`: `Listed`, `Page`, the four list methods, the
   allowance. Tests against recorded payloads: a 304 is `Unchanged`, a
   second page is unconditional, an issues page drops pull requests,
   `Link` yields `last_page`. Commit.
6. `perch-github`: `open_subjects_in`, `discussion_stale`,
   `makes_attention`. Tests on a temporary cache. Commit.
7. `perch-app`: `sync::list_pass` and the ETag rule. Tests with a fake
   client: a new number makes an item; a known number updates its
   subject; an older `updated_at` does not; a missing number is fetched
   and stored closed; an upsert failure leaves the ETag alone. `perch
   --sync`. Commit.
8. Backend: the pass in the poll loop, `attend` narrowed, closure
   enqueues. Commit.
9. `perch-app`: `sync::backfill`, the cursor, the allowance wait, the
   event. `perch --backfill`. Tests with the fake client: step 1 before
   step 2; a restart resumes at the page; the wait triggers under the
   floor. Commit.
10. Status bar line. README: the knowledge section says what the
    backfill covers and names the command. Commit.

Steps 1 through 4 are the model change and land before the sync.
Steps 5 and 6 are independent. Step 9 depends on 6 and 7, not on 8.

## Acceptance

- A config with `[[project]]` fails to load with a message naming
  `[[repo]]`.
- Declare `slatedb/slatedb` and `slatedb/slatedb-go` with no items.
  Launch. Within one poll the sidebar has one section per repository,
  in config order, listing every open pull request and issue on
  github.com in `updated_at` order, with no Priority entries. Rows say
  `#57 · branch`. Group by Owner; one `slatedb` section with both
  inside and rows saying `slatedb/slatedb #57 · branch`. `perch --items`
  agrees with the window.
- Open a checkout under a root whose remote is a repository not in
  config. Its section appears after the declared ones, looks the same,
  and the row menu has no Move entry.
- Ask an agent about a pull request. The thread lands under
  `knowledge/slatedb/slatedb/perch/pulls/`, the root index lists the
  repository, and the skill's `rg` example points at it.
- Poll again with nothing changed upstream. `perch --sync` prints
  "unchanged" and the log shows two requests for the repository.
- Merge a pull request on github.com. Next poll, its row shows Merged
  under Closed and is gone from Active. Its discussion is archived one
  last time and the knowledge file has the merge event.
- Open a pull request on a branch that has a checkout under a root. The
  branch row becomes the pull request row on the next poll, with its
  threads intact.
- Be mentioned on an issue. Needs attention shows the issue with
  "mentioned". Push to a pull request you are only subscribed to. The
  row moves up under its repository and shows "subscribed", as the inbox
  on the web does.
- Kill Perch during a list pass after the 200. Relaunch. The next pass
  asks unconditionally and the items it was upserting are present.
- Run `perch --backfill slatedb/slatedb --limit 20`. Twenty discussions
  land under knowledge, oldest closed first after the open ones, and
  the allowance printed goes down by about four per pull request and
  one per issue. Run it again; it resumes at the cursor, not the start.
- Let the backfill run to the end. The knowledge directory has a file
  for every pull request number github.com lists as closed, and the
  status line is absent. Restart; nothing is refetched.
- Set the floor high enough to trigger. The backfill waits until the
  reset and the list pass keeps running during the wait.
- Go offline. The list pass fails, the poll status says so, the sidebar
  paints from the cache, and the backfill logs and waits.
- `cargo test` green, `cargo clippy` clean.

## Things the implementer should not do

- Do not put closed history into `items`. Knowledge is the record of
  what closed; `items` is what is open and what the reader touched. A
  closed item exists only because it was open in Perch once.
- Do not write the ETag before the upserts. The order is the guarantee.
- Do not make the list pass conditional on the notification poll, or
  the other way round. They are two requests to two endpoints and each
  is right on its own.
- Do not fetch a closed item inline in the list pass. Enqueue it. The
  pass is two requests per repository when nothing changed and stays
  that way.
- Do not give a new item attention, a Priority row, or a badge. It is a
  row. The reader declared the repository and asked to see it.
- Do not add a per-item refresh cadence. Open, notification, and the
  backfill's staleness check are the three triggers.
- Do not sort the backfill newest first for history. Oldest first means
  an interrupted run has a contiguous prefix, and `updated_at` ascending
  is what makes the `since` resume correct.
- Do not run the backfill before the first list pass. It reads the
  items the pass makes.
- Do not let the backfill hold the poll loop. It is a task with a clone
  of `Ctx`; the loop does not await it.
- Do not add GraphQL, a request budget, or an App. If the allowance is
  the problem, the floor is the knob.
- Do not keep a project table, an implicit flag, or a name-to-id
  lookup under another name. The repository's full name is the id.
- Do not draw an undeclared repository differently. It is a repository
  with items; declaring it adds the list pass, not a look.
- Do not migrate the old knowledge layout. Write the new one beside it
  and tell the reader to delete the old.
- Do not migrate the database. Bump and recreate, and say what is lost.
