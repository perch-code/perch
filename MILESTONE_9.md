# Perch: Milestone 9, The repository is the unit

## Goal

Make every open pull request and issue in a declared repository an item,
without anyone having to be notified about it, and fill knowledge with
the whole of a repository's discussion, open and closed.

Today an item comes from one of three places: a checkout found under a
root, a notification the reader opened, or a review request, which is
the one notification that makes an item on its own. Everything else a
repository is doing is in the inbox or nowhere. The sidebar for slatedb
shows three items; github.com shows twenty-odd open pull requests.
Knowledge holds the discussion of whatever was opened in Perch, and
nothing else.

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
- Closed history in the sidebar. Closed pull requests and issues the
  reader never touched go to knowledge and not to `items`. The list is
  sent whole to the window on every change, and a repository's whole
  history in it would be thousands of rows serving a status filter.
- Repositories that are not declared. An item in an undeclared
  repository, made by opening a notification or by a checkout under a
  root, is refreshed on open as today. The list pass runs over
  `project.repos` and nothing else.
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
   makes it take one today; else made new in the repository's project.
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
of now, and no attention. It is a row under its project, sorted by its
subject's `updated_at`, and nothing marks it as new. Priority is still
about attention.

The pass runs inline in the poll loop before `list_threads`, so a
notification about a pull request the pass just made finds the item.

### Attention

`attend` no longer makes items. Every branch that did is gone and the
review-request case with it, because the list pass has already made the
item. What remains is the attention rule, and it narrows:

| Reason | Effect |
|---|---|
| `review_requested`, `mention`, `team_mention`, `assign`, `comment`, `author` | attention, with that reason |
| `ci_activity` | refresh the open pull request's checks, as today; attention with reason `ci_failed` when the checks fail |
| `subscribed`, `state_change`, everything else | nothing |

Subscribed activity on a repository the reader follows is already
reflected by the list pass moving the item's `updated_at`, which the
default sort shows. Making it attention would put every push on every
pull request into Priority.

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
   ingested into knowledge under the repository's project, and not
   stored as an item. When the last page is reached the cursor records
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
`knowledge/<project>/github/<owner>/<repo>/pulls/` holds every pull
request the repository has had, and `issues/` every issue.

The skill text says so. "Discussion history for slatedb from GitHub pull
requests and issues" becomes accurate rather than aspirational, and the
sentence about coverage says that closed history is complete once the
backfill has run and names the command that shows whether it has.

## Model

`perch-github`:

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

`meta` gains, per repository:

- `etag:pulls:<owner/repo>`, `etag:issues:<owner/repo>`.
- `backfill:pulls:<owner/repo>`, `backfill:issues:<owner/repo>`, each a
  page number while paging and `done:<updated_at>` after.

No table changes. The schema version does not bump.

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

1. `perch-github`: `Listed`, `Page`, the four list methods, the
   allowance. Tests against recorded payloads: a 304 is `Unchanged`, a
   second page is unconditional, an issues page drops pull requests,
   `Link` yields `last_page`. Commit.
2. `perch-github`: `open_subjects_in`, `discussion_stale`,
   `makes_attention`. Tests on a temporary cache. Commit.
3. `perch-app`: `sync::list_pass` and the ETag rule. Tests with a fake
   client: a new number makes an item; a known number updates its
   subject; an older `updated_at` does not; a missing number is fetched
   and stored closed; an upsert failure leaves the ETag alone. `perch
   --sync`. Commit.
4. Backend: the pass in the poll loop, `attend` narrowed, closure
   enqueues. Commit.
5. `perch-app`: `sync::backfill`, the cursor, the allowance wait, the
   event. `perch --backfill`. Tests with the fake client: step 1 before
   step 2; a restart resumes at the page; the wait triggers under the
   floor. Commit.
6. Status bar line. Skill text. README: the projects section says
   declared repositories are listed whole; the knowledge section says
   what the backfill covers and names the command. Commit.

Steps 1 and 2 are independent. Step 5 depends on 2 and 3, not on 4.

## Acceptance

- Declare slatedb in config with no items. Launch. Within one poll the
  sidebar's slatedb section lists every open pull request and issue on
  github.com, in `updated_at` order, with no Priority entries. `perch
  --items` agrees with the window.
- Poll again with nothing changed upstream. `perch --sync` prints
  "unchanged" and the log shows two requests for the repository.
- Merge a pull request on github.com. Next poll, its row shows Merged
  under Closed and is gone from Active. Its discussion is archived one
  last time and the knowledge file has the merge event.
- Open a pull request on a branch that has a checkout under a root. The
  branch row becomes the pull request row on the next poll, with its
  threads intact.
- Be mentioned on an issue. Priority shows the issue with "mentioned".
  Push to a pull request you are only subscribed to. Priority does not
  change; the row moves up under its project.
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
- Do not make attention from `subscribed`. That is the whole reason the
  inbox was noisy.
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
- Do not bump the schema. Four `meta` keys are enough.
