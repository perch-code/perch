# Perch: Milestone 20, Local work in the inbox

## Goal

When an agent finishes on one of your branches, the only sign today is
a toast (`AgentCompletionNotice`) that goes away. If you were looking
at another item, or at another app, the work sits there until you
remember to check. GitHub activity doesn't have this problem: it stays
in Notifications until you mark it Done, and comes back when something
newer happens. After this milestone, local work behaves the same way.

- When an agent finishes on an item, or a worktree's head moves while
  you're away, the item appears in Notifications as unread. Opening it
  marks it read. **Done** removes it. Newer activity brings it back.
- An item has **one** row in Notifications. A branch you pushed and
  opened a PR for is one item already (`attach_pull_requests`), so its
  GitHub thread and its local activity share a row, not two.
- Branches of your own show up in **Authored**. Today no branch-only
  item ever does.
- An item whose worktree and PR have different heads says so, with
  ahead and behind counts. Merge and Submit review are blocked while
  they differ, which today holds only for Perch's own PR checkouts and
  not for your worktrees.

## Non-goals

- macOS notifications. This is the in-app inbox only.
- Knowing when an agent Perch didn't launch has *finished*. Perch sees
  such an agent only through its commits, as head moves on the root
  scan. An agent that edits and never commits shows nothing until
  it does.
- Telling a human's commit from an agent's. A head move you made
  yourself in a terminal also shows up. Done is one click, and guessing
  from commit trailers would be wrong often enough to be worse.
- Per-item muting, an equivalent of Unsubscribe for local activity.
  Follow-up if a busy branch gets noisy.
- Syncing anything local to GitHub. Done on a merged row marks the
  GitHub thread done, as Done does now. Local activity never reaches
  github.com.
- Moving a worktree to the PR's head, or the other way round. Perch
  reports the divergence and blocks what would act on the wrong
  commits. It doesn't fix it.

## Behaviour

### What counts as activity

Local activity is recorded on the item, not on a run. Each new event
replaces the item's previous one. These are events:

- **An agent run on the item ends**: Ask or Work, `Discussion` owner,
  status `Completed`, `Failed` or `Interrupted`. Runs that end with
  `UserStopped`, `QueueCancelled`, `AppShutdown` or `AppRestart` don't
  count: you stopped them, or you quit.
- **A worktree review ends**: once, when the last of its reviewer runs
  is terminal, not once per reviewer.
- **The root scan sees a worktree's head move**: `scan_roots` finds an
  unowned checkout whose `head_sha` differs from the stored one, and
  the new head isn't `last_seen_head`. This catches Claude Code and
  Codex sessions running in your worktrees. The scan runs with every
  notification poll, and at launch, so a branch that moved while Perch
  was closed shows up when it opens.
- **The root scan finds a new worktree that already has commits past
  its base**: an agent made the worktree and committed between polls.
  A new worktree still at its base is a row in All, as now, and not
  activity.

Nothing is recorded while the item is open in the main pane and
Perch's window is key: you watched it happen. Nothing is backfilled
when the schema migrates. Existing items start with no activity, so
the first launch doesn't flood the inbox.

A Perch Work run that commits in a root worktree also moves its head.
That's one event, not two: the scan sees the head the run's own event
already recorded, and doesn't raise another.

### Read and Done

These work as they do for a GitHub thread, keyed on the activity's
time instead of `updated_at`.

- **Opening the item** marks its activity read, from any row: the
  notification's, or the item's in All or Authored. A read row stays
  in Notifications, not bold, as a read thread does.
- **Mark Unread**, on the notification's row or the item's, puts the
  unread mark back for the current activity.
- **Done** hides the activity. Newer activity brings the row back. Done
  never archives, deletes or touches the item or its worktree. Archive
  removes a branch's directory, which is the last thing clearing an
  inbox row should do.
- An item that's archived or deleted keeps no row: deleting drops its
  activity with it. An archived item's row stays until Done, and
  opening it restores the item, as opening does now.

### One row per item

The Notifications list becomes GitHub threads and local activity
together. For each item with visible local activity:

- If a visible GitHub thread `is_about` the item's subject, that
  thread's row is the item's row. It's merged: unread if either side
  is unread, its time the later of the two, and the byline shows the
  local activity beside what it shows now.
- Otherwise the item gets a row of its own.

A thread with no local activity is drawn as it is today. A thread
about a subject Perch has no item for is drawn as it is today.

On a merged row:

- Opening reads both: the thread through `read_subject_notifications`,
  as now, and the local activity.
- Done does both: marks the thread done on GitHub, as now, and hides
  the local activity. Each comes back on its own newer activity, and
  either one brings the row back.
- Mark Unread marks both.
- Unsubscribe applies to the thread only, as now.

The list keeps GitHub's order and fits local rows in by time. GitHub
returns threads by `updated_at`, newest first, so a merge on the row's
time keeps that order. Local activity arriving goes through the same
hold-back as a poll does: rows already on screen don't move, and the
pill over the list counts the new ones.

### The row

A local row has the shape a notification row has:

- **Glyph**: the item's own, as in All: branch, or the PR's state.
- **Title**: the item's title, or its Rename.
- **Trailing**: the unread dot and the age, and on hover Done and the
  menu (Done, Mark Unread).
- **Byline**: the repository, the branch where a number would go (or
  the number for a PR item), then an activity chip, then the churn:

      ⎇ fix-sidebar-width                            ● 4m
      [face] perch · fix-sidebar-width  Claude · Work · 3 files  +42 −7

The activity chip is the latest event:

- a run: `Claude · Work · 3 files`, `Codex · Ask`, or in the failure
  colour `Claude · Work · failed`
- a review: `Review · 2 findings`
- a head move: `2 new commits`

A merged row shows the chip after its review pill. Once the item has
a divergence (see below), the byline also shows `3 unpushed` or
`2 behind PR`.

The reason filter gets an **Agent** reason, covering rows with local
activity. A merged row matches Agent and its thread's own reason. The
repository filter uses the item's `repo_full_name`, so `local/<name>`
repositories can be picked.

### Where opening a row lands

A local row, or a merged row whose latest side is local, opens the item
at what happened:

- **A Work run that changed files**: Code, on that run's **What the run
  changed** scope.
- **A Work run that changed nothing, an Ask run, or a failure**:
  Conversation, with the run's thread card focused (`focus_card`).
- **A review**: Conversation, scrolled to the review's group in REVIEW
  HISTORY.
- **A head move**: Code, on **Since you last looked**. Opening then
  moves `last_seen_head`, as it does now.

The run and review cases pick a scope for this one open. They don't
change the scope the item remembers from the menu, just as opening a
run from its card doesn't.

A merged row whose latest side is GitHub's opens as a notification does
now.

### Item rows in All and Authored

The unread dot and bold title on an item's row (`ui/items.rs`,
`unread`) come from the inbox's GitHub thread or from the item's own
local activity. A branch can now be unread. Mark Unread in the item
row's menu covers local activity too. It's enabled when either side is
read.

### Authored

`ItemScope::Authored` accepts an item with no subject. A branch-only
item is always one of yours:

- Perch made it (New work, Start work).
- Or the root scan found it under one of your roots. The scan skips the
  base branch, so `main` never becomes an item.

A PR item still goes by the PR's author. A local copy of someone else's
branch turns into their PR item once `attach_pull_requests` finds the
PR, and drops out of Authored. The one miss is a coworker's branch
checked out with no PR yet, which counts as yours. That's rare enough
to accept.

### Divergence

An item with both a checkout and a PR has two heads: the worktree's
`HEAD` and the PR's head. Each kind of activity is tied to its own
revision, so they never have to agree:

- A run's changes are its `refs/perch/runs/<run>/{before,after}`
  snapshot, whatever either head does after.
- GitHub activity (reviews, threads, CI) is about the PR's head.

When the heads differ, it's a status on the item, not a conflict to
resolve. Perch shows:

- **Worktree ahead**: `3 unpushed`. This is the usual state of your own
  branch after an agent commits.
- **PR ahead**: `2 behind PR`. Someone pushed from elsewhere, or a bot
  or reviewer pushed a fix.
- **Diverged**: `3 unpushed · 2 behind PR`. There was a rebase or
  force-push on one side.
- **PR head unknown here**: the PR's head commit isn't in the local
  repository, so there are no counts. The status reads `PR head not
  fetched`, and the heads are still known to differ.

The status shows in the summary's Branch row, and in the byline in
Notifications, All and Authored.

Perch's own PR checkouts keep following the PR head as they do now:
they fast-forward when the tree is clean and the commit on disk is one
the PR had. Your worktrees, the ones `open_own_checkout` opens, are
never moved, reset or fetched into, as now.

**Gating.** `open_own_checkout` sets `stale: None` today. So a pushed
branch whose worktree has moved on can be submitted as a review as if
the diff on screen were the PR. Merge and Submit review now refuse
whenever the item's checkout isn't at the PR's head, as they do for a
stale owned checkout, with the divergence status as the reason.

"Since you last looked" keeps following the worktree's head.
`last_seen_head` is the worktree head at the last open, since that's
what Code shows.

## Model

- The item's checkout gains `divergence: Option<Divergence>`, where
  `Divergence { head, pr_head, ahead: u32, behind: u32 }` is the last
  count, persisted in an `items.divergence` column (schema 28).
  `Item::divergence()` returns `Diverged { ahead: Option<u32>, behind:
  Option<u32> }` when the checkout and the open PR are at different
  commits, with counts only while the stored count is for those two
  heads. `perch_git::Checkout.stale` stays as it is: on an owned
  checkout it also means "left at an older head because the tree was
  dirty", which greys the uncommitted cuts and stops the head
  following commits. Merge and Submit review refuse on `stale` or on
  `Item::divergence()`.
- `ItemActivity { item_id, at: Timestamp, event: ActivityEvent,
  read_at: Option<Timestamp>, done_at: Option<Timestamp>,
  local_unread: bool }`, where `ActivityEvent` is one of:
  - `Run { run_id, backend, mode, outcome, files }`
  - `Review { review_id, findings }`
  - `HeadMoved { from, to, commits }`
  - `NewWorktree { head, commits }`

  It's unread when `local_unread`, or when `read_at` is before `at`. It's
  visible when `done_at` is before `at`.
- `AppState` gains `item_activity: RwSignal<HashMap<String,
  ItemActivity>>`, sent with items. `subject_unread` stays for GitHub.
  A new `item_unread(item)` combines the two.
- The notification list's element becomes `InboxRow { Thread(Thread),
  Local(ItemActivity), Merged(Thread, ItemActivity) }`. Its key adds
  the activity's `at` and read state, so a new event on an existing
  thread's row redraws it. `pending_notifications` holds `InboxRow`s.

## Backend

- Schema 29: an `item_activity` table, one row per item, keyed by
  `item_id` with `ON DELETE CASCADE`, holding the fields above with the
  event as JSON. The migration creates it empty.
- `record_activity(item_id, event)`, called from:
  - the terminal path of `execute_question`, after
    `insert_completed_reply`
  - the end of a worktree review
  - `scan_roots`, where it updates a moved head or makes a new row

  It skips when the item is open and the window is key. The UI tells
  the backend whether the window is key, as it tells it what's open.
  It sets `at`, clears `local_unread`, and leaves `read_at` and
  `done_at`, so a new event over a done one is visible again.
- A run event records the worktree's `HEAD` as the run ended, and the
  scan skips a head move to that commit: it's the run's own commit,
  already recorded. (`work_after` is a snapshot under `refs/perch/runs`,
  never the branch head, so it can't serve.)
- The scan only raises head moves and new worktrees in repositories it
  has scanned before, kept in the `activity.scanned_repos` meta key. The
  first scan after the upgrade, or of a newly added repository, records
  where things are without calling any of it news.
- Opening an item sets `read_at = at`. This is the same place
  `read_subject_notifications` runs.
- The commands `ItemActivityDone(item_id)` and
  `MarkItemActivityUnread(item_id)`. Done on a merged row sends
  `DoneNotification` and `ItemActivityDone` together.
- The divergence is computed where `open_own_checkout` and the owned PR
  path already compare heads, and by `scan_roots` for items with a PR.
  Ahead and behind come from `git rev-list --left-right --count
  HEAD...<pr_head>` in the checkout's repository. If the PR's head
  isn't there, the counts are `None`. For your worktrees nothing is
  fetched to find it. `refs/remotes/<remote>/<branch>` is tried first,
  since your own `git push` keeps it current.

## Harness

Headless tests in the style of the ones they sit beside.

- Activity: each run outcome records or skips as listed, a review
  records once for two reviewers, a scan head move records, a scan head
  move matching a run's `work_after` doesn't record again, a new
  worktree records only with commits past its base, nothing records
  while the item is open and the window key, and the migration
  backfills nothing.
- Read and Done: opening from the notification's row and from the
  item's row both read. Mark Unread brings the dot back. Done hides the
  row. A newer event brings it back unread. Done leaves the item, its
  checkout and `archived_at` alone.
- Merging: a PR item with an unread thread and read local activity is
  one unread row. Done on it calls `mark_thread_done` and hides the
  activity. Newer GitHub activity alone brings it back, and so does
  newer local activity alone. A thread for a subject with no item is
  drawn as today.
- Order and hold-back: local rows fit in by time without reordering
  GitHub's rows, and activity arriving while the list is on screen goes
  to the pill.
- Filters: Agent reason, merged rows matching both reasons, and
  `local/<name>` in the repository filter.
- Landing: each event opens where listed, and a run's scope isn't
  remembered as the menu's last pick.
- Authored: a branch-only item is in it, a PR by someone else isn't,
  and a branch that gains someone else's PR drops out.
- Divergence: ahead, behind, both, and PR head not local each produce
  their status. `open_own_checkout` sets the divergence. Merge and
  Submit review refuse on any divergence, including on a root worktree.
  An owned PR checkout still follows the PR head.

## Build order

1. Authored takes branch-only items. It's small and stands alone.
   Commit.
2. Divergence: `Divergence` replaces `stale`, `open_own_checkout` sets
   it, the counts, the Branch row status, and the gating on root
   worktrees. This is a correctness fix and lands before anything else
   leans on it. Commit.
3. Activity recording: the schema, `record_activity` from runs, reviews
   and the scan, the open-and-key suppression, read on open, Done and
   Mark Unread in the backend. There's no UI yet beyond item rows
   showing unread. Commit.
4. The inbox: `InboxRow`, local and merged rows, the byline chip and
   divergence text, order and hold-back, the Agent reason, and the
   landing rules. Commit.
5. README: the inbox section gets local activity, merged rows and what
   Done does and doesn't touch. Authored gets branches. Keeping work
   safe gets the divergence status and gating. Commit.

## Acceptance

- Send a Work run on a branch, then open another item. When the run
  finishes, the branch is at the top of Notifications, bold, with
  `Claude · Work · N files`. Click it: Code opens on What the run
  changed, and the row goes read. Mark it Done: it's gone, and the
  branch and worktree are untouched in All.
- Send another run on the same branch. When it finishes, the row is
  back, unread.
- Send an Ask run and watch it finish with the item open: no row.
- In a terminal, commit in one of your root worktrees while Perch shows
  another item. After the next poll the branch shows `1 new commit`.
  Opening it lands on Since you last looked.
- Let a Claude Code session in `.claude/worktrees/` make a worktree and
  commit. After the next poll it's in Notifications and in Authored.
- Push a branch and open a PR for it. When a review comment arrives
  after a local run, Notifications has one row for it, not two. Done
  clears both. A new comment brings it back. So does a new run.
- Commit in that PR's worktree without pushing. The Branch row reads
  `1 unpushed`, and Merge refuses and says why. Push: the status clears
  and Merge is offered again.
- Force-push the PR from elsewhere: the status reads `N unpushed · M
  behind PR` or `PR head not fetched`. The worktree isn't moved.
- Quit Perch, commit in a worktree, relaunch: the row is there.
- Upgrade a database with existing items: Notifications has no local
  rows until something new happens.

## Things the implementer should not do

- Don't use item Archive for Done, and don't archive, delete or move a
  worktree from any inbox action.
- Don't draw a second row for an item that already has a GitHub
  thread row.
- Don't fetch into, reset or check out anything in a root worktree to
  compute divergence.
- Don't record activity from `last_opened_at` changes, churn changes
  alone, or uncommitted edits. Only run ends, review ends and head
  moves count.
- Don't backfill activity at migration.
- Don't run `cargo fmt --all`. Format only new files.
