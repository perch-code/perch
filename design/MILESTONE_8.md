# Perch: Milestone 8, The item view

## Goal

Give every item one view, shaped like a pull request page on GitHub:
a header, then two tabs. Conversation, and Files.

Today the review view is a file rail, an editor and a thread column, and
the issue view is a column of cards. They were built one after the other
and share a header and not much else. The pull request's own discussion
is fetched, archived under knowledge, and never shown. The threads the
reader has with Claude sit in a column beside the code and nowhere else.
Someone reading a pull request in Perch and a colleague reading it on
github.com are looking at two different things.

After this milestone opening a pull request lands on Conversation: the
description, then every comment, review and line comment from GitHub,
and every thread the reader has had with an agent about it, interleaved
by time in one column of cards. Files is the review view as it stands,
with a scope menu above the rail that narrows the diff to one commit,
to the uncommitted work, or to what changed since the reader last looked;
a Viewed check on each changed file; a split layout; and references from
rust-analyzer. An issue is the same view with Conversation and nothing
else, and a branch of the reader's own is the same view with a
Conversation of only local threads.

GitHub has a Commits tab because GitHub has no editor. Perch has one, so
a commit is not a place to go but a scope to read the files under. That
is why there are two tabs and not three, and why the commit list is a
menu in the rail rather than a pane of its own.

The design decision the milestone turns on is that a local thread and a
GitHub comment are two things and must never look like one. Perch does
not write to GitHub. A thread the reader had with Claude is private to
this machine, and a card that could be mistaken for a comment on the
pull request would be a lie. Both kinds share a timeline. They do not
share a face.

## Non-goals

- Writing to GitHub. There is no reply box under a GitHub comment, no
  review, no approve, no resolve. The only composer is the one that asks
  an agent. Viewed is Perch's own, like Saved and Done.
- A Commits tab. The commit list is the scope menu.
- A Checks tab. Workflow runs are already fetched for CI notifications.
  A pass or fail pill in the header that opens the run in the browser is
  the whole of it.
- Editing the sidebar on the right of GitHub's page. Reviewers, labels,
  milestone and linked issues are shown as a strip under the header,
  read-only, and only what the payload already carries.
- Editing in Perch. Files is still read-only.
- Commit ranges. The scope is one commit, the uncommitted work, the
  since-last-looked range, or the branch. "From here" and "to here" on a
  commit row are the way to add ranges later, if a single commit turns
  out not to be enough.
- Call hierarchy. References (find usages) ships. Incoming and outgoing
  calls need a tree panel of their own and wait until references has
  been used for a while.
- Rendering a GitHub suggestion block as a diff, or a reaction, or an
  edit history. Comment bodies render as the markdown they are.
- Live updates to the conversation. It refreshes when the item is opened
  and when a notification about the item arrives, the way the pull
  request does today.
- A word-level diff. Changed lines are tinted as lines, in both layouts.

## Behaviour

### The view

Header, a tab strip, then the body of the selected tab. The header is the
one that exists, with additions named below. The tab strip sits where the
issue view's cards begin today and where the review view's rail and tab
bar begin, and it is the same strip on every item:

| Tab | Shown for | Body |
|---|---|---|
| Conversation | every item | the timeline |
| Files | items with a checkout | the review view |

A tab carries a count in the muted colour: comments and threads for
Conversation, changed files under the current scope for Files. An issue
item shows one tab, and the strip is still drawn so the view does not
change shape when Start work adds a checkout.

Opening an item lands on Conversation. The tab is remembered per item for
the life of the app, so coming back from the sidebar reopens the tab that
was open. `Cmd-Shift-1` and `Cmd-Shift-2` select the tabs. `Cmd-[` remains
back.

### The header

What is there stays: back, the icon and title, the notice, the branches,
the directory, churn, Done. Added:

- A state pill before the title: Open, Draft, Merged, Closed for a pull
  request; Open, Closed, Not planned for an issue; nothing for a bare
  branch. The colours are the ones the sidebar glyphs already use.
- A checks pill after the branches, for items with a pull request and a
  head sha: a check, a cross or a dot in the muted colour for passing,
  failing and running. Hover names the workflow that failed. Click opens
  the run on github.com. The pill is absent when nothing has been fetched
  rather than shown grey.
- Under the bar, a strip in the muted colour: reviewers with their
  verdict glyph, labels as chips in their colour, assignees, milestone,
  and the issues the pull request says it closes. Each part is absent
  when empty. The strip is absent when all of them are. An issue's strip
  is what its view shows today, moved, plus the "In Perch" branches and
  pull requests as chips.

The churn in the header follows the scope on Files. On Conversation it is
the branch's.

### Conversation

One column, the width of the issue view's, of cards in time order. Five
kinds of card:

- **Description.** The pull request or issue body, led by who opened it
  and when. Always first. "No description." when empty.
- **Comment.** One GitHub conversation comment. Avatar, login, "commented",
  time, body as markdown. A link glyph at the right of the byline opens
  it on github.com.
- **Review.** A review with a verdict: "approved", "requested changes",
  "commented", with the verdict's glyph in the byline. The review's
  summary, if any, then under it the line comments that belong to the
  review, each as a nested block headed by `path:line` and showing the
  anchored lines of code, tinted as the diff tints them, then the
  comment's body. Replies nest under the comment they answer, flush with
  it. Click the path and Files opens on that file, scrolled to the line.
- **Event.** One line, no card chrome, for what the pull request did:
  opened, converted to draft, ready for review, merged by whom, closed,
  reopened. Only what the pull request payload and the reviews already
  carry. Merged and closed come from `merged_at` and `closed_at`.
- **Thread.** A local thread with an agent. See below.

Cards are ordered by the time of their first message. A review's line
comments stay inside their review regardless of their own time. A thread
sorts by the time of its question.

The column ends with the composer for asking about the item as a whole,
the same composer the thread column opens on `a` with no file selected,
laid in place rather than in a column. `a` on Conversation focuses it.
While a run is in flight the new thread card appears above the composer
and fills as it goes.

An issue's Conversation is the issue view as it stands: description,
comments, threads, and the composer.

A bare branch's Conversation is its threads, and the composer. "Nothing
here yet. Ask about the branch below." above the composer when there are
none.

### Thread cards

A local thread is a card with a different face from a GitHub comment,
and the difference is not subtle:

- The background is the canvas colour, not the card colour, and the
  border is dashed.
- The byline is the reader's own avatar and login, then "asked
  Fable · high", the model and effort as the thread column shows them,
  then the time, then "Local" in a chip in the muted colour. Hover on the
  chip: "This thread is on this machine and is not on GitHub."
- Under the byline, for an anchored thread, the anchor block: `path:41–58`
  and the lines, tinted, exactly as a review comment's nested block shows
  its lines. Click it and Files opens there. An outdated anchor says
  Outdated where the thread column says it now.
- Then the question, then the run's step lines collapsed to one
  ("Read 4 files, ran 2 commands", click to expand), then the answer as
  markdown, then cost and time. Then a follow-up box, which is the
  composer for this thread.

A thread anchored to lines shows in two places: as a card on Conversation
and in the thread column on Files, as today. They are one thread. A
follow-up sent from either shows in both. A thread about the item as a
whole shows on Conversation, and in the thread column only when the
column is showing threads from elsewhere, as it does now.

Deleting a thread is from the card's menu, with the confirmation the
thread column uses.

### Asking about a GitHub comment

A GitHub card has no reply. It has "Ask about this" on hover in the
byline, which opens the composer with the comment quoted above the
draft, as a markdown quote with the author and a permalink, and for a
line comment with the anchor set to the comment's lines. The thread that
results is a local thread, and its card says so. The quoted comment is
in the question so the agent has it; the thread does not otherwise point
back at the GitHub comment.

### Files

The review view as it stands: the rail on the left, the file tabs and
the editor, the thread column on the right. Changed here:

**The scope menu.** A button at the top of the rail, above the
Changed/All files toggle, showing the current scope's name and its
churn: "Branch  +98 −26". Click opens a menu:

| Entry | Meaning | Greyed when |
|---|---|---|
| Since you last looked | `<head at last open>..HEAD` | the head has not moved, "No new commits" |
| Uncommitted | the working tree against `HEAD` | the checkout is clean |
| ‣ Unstaged | the working tree against the index | nothing unstaged |
| ‣ Staged | the index against `HEAD` | nothing staged |
| Committed ▸ | a submenu, one row per commit over the base, newest first: short sha, subject, relative time; a check on the selected one | no commits over the base |
| Branch | `<base>..HEAD`, the range the diff uses today | never; checked by default |

Uncommitted, Unstaged and Staged appear only for a checkout that is on
disk. A pull request whose checkout was left dirty because the head
moved (the `stale_head` notice today) opens with Uncommitted greyed out
and the notice explaining why, as now.

The scope is per item and forgotten when the item is closed, except that
"Since you last looked" is what the head was at the previous open, and
that is updated on every open as `last_opened_at` is today.

Every scope produces the same shape: a list of changed files with
status and churn, and for each file a base and a head to diff. The rail,
the tree's change markers, the editor's tints, the change nav, the tab
strip's count, the header's churn and Viewed all read that shape and do
not know which scope made it. The whole-tree mode is unaffected: it lists
every file, and marks the ones the scope changed.

**Viewed.** Each row in the changed list has a checkbox at its right
edge. Checked, the row drops to the muted colour and the check stays.
Above the list, "3 of 12 viewed" in the muted colour, and a "Mark all"
that flips to "Clear all". `v` toggles Viewed on the open file. `}` and
`{` in the editor go to the next and previous unviewed file, opening it,
the way `]` and `[` go between hunks.

Viewed is on a file's content, not on a commit. It is keyed by item,
path and the blob sha of the head side. A file checked under one scope
is checked under any scope whose head side is the same blob. A push that
changes a file clears its check, as it does on GitHub, and a file that
comes back to a blob that was viewed is viewed. Uncommitted edits to a
viewed file show it unviewed until the edit is undone or committed and
viewed again.

Viewed is Perch's own and stays on this machine. It does not sync with
GitHub's Viewed, in either direction, for the reason Saved and Done do
not: nothing lists it back.

**Layout toggle** in the file tab bar's right end, beside the change
nav: Stacked and Split. Stacked is what exists. Split draws the base
file on the left and the head on the right in two editors that scroll
as one, removed lines tinted on the left and added lines on the right,
with unchanged lines aligned and a gap filled on the side that lacks
them. The gutter on each side carries that side's line numbers.
Selection, `a`, the `+` in the gutter, hover and go-to-definition work
on the right side; the left side takes no anchors. The toggle is
remembered across items.

**References.** In the hover panel, beside "Go to definition", "Find
references". It asks rust-analyzer for references to the symbol under
the cursor, and the answer is a panel at the bottom of the editor,
resizable, listing one row per reference: `path:line` and the line's
text with the symbol emphasised, grouped by file with a count. Click a
row and the editor opens there. The panel has a close glyph and closes
on `Escape` when nothing else claims it. `Shift-F12` from the keyboard
on the cursor's symbol. References in files the scope changed carry the
change marker the rail uses.

**Threads from GitHub.** A review line comment shows in the gutter with
a marker of its own, hollow where a local thread's is solid, and in the
thread column under the local threads for the file, as the nested block
Conversation shows, with "Ask about this". The column's heading
distinguishes the two groups: "Threads" and "On GitHub".

### Checks

Nothing new is fetched on a schedule. When an item with a pull request is
opened, one request lists the workflow runs for the head sha, and the
header pill is set from the worst conclusion among them. A CI notification
for the item refreshes it. Nothing is shown when the request fails.

### Knowledge

The discussion archived under knowledge already carries every GitHub
message. Local threads are added to it: under the same pull request or
issue directory, a `perch/` file per thread with the question, the
answer, the anchor and the model, marked as Perch's and not GitHub's in
its front matter. The skill text says the two are different and which is
which. An agent asked "why was this done" can find the reader's own past
questions about the same lines.

## Model

`perch-github`:

- `Discussion` gains `events: Vec<PullEvent>`, derived at fetch time from
  the pull request payload: `Opened`, `Merged { by }`, `Closed`,
  `Reopened`, `Draft`, `Ready`, each with a time. Nothing new is fetched
  for them in this milestone; the timeline endpoint is later.
- `ReviewComment` gains `diff_hunk: String` and `side: Side`, both in the
  payload already. `diff_hunk` is what the nested block shows when the
  anchored lines are not in the checkout.
- `PullRequest` gains `merged_at`, `merged_by`, `closed_at`, `draft`,
  `requested_reviewers`, `milestone`. All in the payload today and
  dropped.
- `Checkout` gains `last_seen_head: Option<String>`, the head at the
  previous open, set when the item is opened from `head_sha` before
  `head_sha` is refreshed.
- A `CheckState { Passing, Failing { workflow }, Running, Unknown }` and
  `fn worst(runs: &[WorkflowRunPayload]) -> CheckState`.

`perch-git`:

- `Commit { sha, subject, body, author, authored_at }` and
  `log(base, head) -> Vec<Commit>` on `Git`.
- `Scope`, the enum of what the menu offers:

  ```rust
  pub enum Scope {
      Branch,                       // base..HEAD
      SinceLastLooked { from: String },
      Commit { sha: String },       // sha^..sha
      Uncommitted,                  // HEAD..worktree
      Unstaged,                     // index..worktree
      Staged,                       // HEAD..index
  }
  ```

  and `Scope::sides(&self) -> (Rev, Rev)` where `Rev` is `Sha(String)`,
  `Index` or `Worktree`. `changed_files(scope)` and `file_diff(scope,
  path)` take a scope, and read each side with `git show <sha>:path`,
  `git show :path` or the file on disk. The existing `base_ref..head`
  callers become `Scope::Branch`.

`perch-app::review::model`:

- `ReviewState` gains `tab: RwSignal<ItemTab>`, `layout:
  RwSignal<Layout>`, `scope: RwSignal<Scope>`, `commits:
  RwSignal<Arc<Vec<Commit>>>`, `dirty: RwSignal<Dirt>` where `Dirt` says
  whether there is anything unstaged and anything staged, `discussion:
  RwSignal<Option<Arc<Discussion>>>`, `checks: RwSignal<CheckState>`,
  `viewed: RwSignal<Arc<HashSet<(String, String)>>>` of path and blob,
  `references: RwSignal<Option<Arc<References>>>`.
- A pure `timeline(discussion, threads, issue_comments) -> Vec<Card>`
  where `Card` is the enum of the five kinds above, sorted. This is
  where the interleave lives and it is tested without a window.
- `ChangedFile` gains `head_blob: Option<String>`, the blob sha of the
  head side, `None` for a deletion. Viewed is looked up by it.
- `LoadedFile` gains `base_buffer: Option<DisplayBuffer>`, filled when
  the layout is Split, and `alignment: Vec<Row>` where a `Row` is
  `(Option<usize>, Option<usize>)`, a base line and a head line, computed
  from the same `FileDiff` the stacked view uses.
- `LoadedFile` carries the scope it was diffed at, so a scope change
  invalidates loaded files and they reload rather than repaint stale.

## Storage

The discussion is fetched today and handed to knowledge without being
kept. It is kept now:

- `discussions (item_id TEXT PRIMARY KEY, fetched_at TEXT, json TEXT)`.
  One row per item, replaced wholesale on fetch. Conversation paints from
  it before the network answers, the way the inbox does.
- `checks (item_id TEXT PRIMARY KEY, head_sha TEXT, fetched_at TEXT,
  json TEXT)`. Replaced on fetch. A row whose `head_sha` is not the
  item's is ignored and refetched.
- `viewed (item_id TEXT, path TEXT, blob TEXT, viewed_at TEXT, PRIMARY
  KEY (item_id, path, blob))`. Rows are never cleared by a push; a
  changed file simply has a blob with no row. Deleted with the item.
- `items` gains `last_seen_head TEXT`.
- `comment_threads` is unchanged. A thread made from "Ask about this"
  carries nothing that ties it to the GitHub comment. The quote is in the
  message body.
- `meta` gains `review.layout` for the Stacked/Split toggle.

The schema version bumps and the database is recreated, as Milestone 7
established. Threads are lost. This is the last milestone that says so
without a migration; the next one that changes the schema writes one.

## Backend

- `OpenItem` for a pull request item also fetches the discussion and the
  workflow runs for the head, after the checkout is ready, and emits
  `DiscussionReady(item_id, Arc<Discussion>)` and `ChecksReady(item_id,
  CheckState)`. The cached row is emitted first if present. Knowledge
  ingest runs off the same fetch, as today.
- `OpenItem` for any item with a checkout runs `git log` for the branch
  range and `git status --porcelain` and emits `CommitsReady(item_id,
  Arc<Vec<Commit>>, Dirt)`. `last_seen_head` is written from the old
  `head_sha` before the head is refreshed.
- `SetScope(item_id, Scope)` sets the scope the diff commands use for
  that item. The changed-files list is recomputed and re-emitted with
  `head_blob` on each row; open files reload. The list for `Uncommitted`
  and its children comes from `git diff --name-status` against the right
  sides, and untracked files are included as added.
- `OpenFile` reads the scope and both sides from it. It gains
  `with_base: bool`; when set, the base content is returned alongside
  and the alignment is computed in the backend from the diff, not in the
  view.
- `SetViewed(item_id, path, blob, bool)` writes or deletes the row and
  emits the set. `ViewedReady(item_id, set)` on open.
- `References(token, path, line, character)` asks the LSP client for
  `textDocument/references` with `includeDeclaration: true` and emits
  `References(token, Vec<Location>)`. Locations are read for their line
  text in the backend so the panel needs no file loads.
- `Ask` gains nothing. The quote for "Ask about this" is composed in the
  view and is part of the prompt.
- The knowledge writer gains `threads_to_messages(item, threads)` and is
  called when a run finishes.

`perch-lsp` gains `references(path, line, character) -> Vec<Location>`,
beside `hover` and `definition`.

The LSP client is opened on the worktree. Under `Uncommitted` the editor
shows the file on disk, which is what rust-analyzer sees, so hover and
definition are right. Under a commit scope the head side is `git show`
output, which may not match the disk; hover and definition still run
against the disk and may be off by the lines that differ. This is the
same limit the stale-head notice has today, and the notice covers it:
"Showing 291baec; rust-analyzer sees the working tree."

## UI

### The strip

`review::strip`, a horizontal bar under the header, height `TAB_HEIGHT`,
two tab labels with counts and an accent underline on the active one,
drawn the way the file tabs draw theirs so the two strips read as
related.

### Conversation

`review::conversation`, built from the issue view. `issue::card`,
`byline`, `description_card` and `comment_card` move to a
`review::cards` module and gain the review, event and thread cards. The
issue view becomes a caller of it.

The thread card and the thread column render the same thread from the
same `CommentThread`. The message body, the step lines and the footer are
one set of views taking a `ThreadFace { Column, Card }` for the sizing
differences. Do not write them twice.

### The scope menu

`review::scope`, the button and the menu, using the item menu's popup
and submenu from Milestone 7. The greyed entries say why on hover in one
line, as the item menu's do. The button's churn is the changed list's
total.

### The rail

`rail::mode_toggle` gains the Viewed count row above the changed list,
and each changed row gains its checkbox. The tree list is unchanged but
for reading `head_blob` to grey viewed files.

### Split view

`review::editor` takes a `Layout`. Stacked is untouched. Split is two
`editor` instances over one scroll signal, each given a side and the
alignment, drawing a filler row where its side has no line. The gutter
takes a side. Hover, selection and anchors are wired to the right editor
only.

### References panel

`review::references`, a bottom panel under the editor with the resize
handle the thread column uses, turned sideways. Rows are the changed-list
rows with a second line for the text.

## Harness

- `perch --timeline <item-id>` prints the cards in order, one line each,
  kind and time and first line, so the interleave can be read without a
  window.
- `perch --scope <item-id> <scope>` prints the changed list for a scope,
  with status, churn and head blob, and the viewed mark.
- The `hover` example in `perch-lsp` gains a `references` mode.

## Build order

1. `perch-git`: `Scope`, `Rev`, `Commit`, `log`, and the diff commands
   taking a scope. Tests on a temporary repository with three commits,
   a staged edit and an unstaged one: each scope lists the right files
   and the right sides. Commit.
2. `perch-github`: the payload fields, `CheckState` and `worst`,
   `last_seen_head`, the three tables, the schema bump. Tests: `worst`
   over every conclusion; the discussion round-trips through its row;
   viewed is keyed by blob. Commit.
3. `perch-app`: `Card`, `timeline`, pure and tested: a review's line
   comments stay inside it; replies nest; a thread sorts by its question;
   events land between comments; an issue has no events. `perch
   --timeline`. Commit.
4. Backend: keep and emit the discussion, the commits, the dirt, the
   checks and the viewed set on open; `SetScope`; `SetViewed`;
   `OpenFile` over a scope. `perch --scope`. Commit.
5. UI: the strip, and Conversation over the issue view's cards, for pull
   requests, issues and bare branches, with the thread card sharing its
   views with the column. The composer in place. "Ask about this". The
   header pills and the metadata strip. Commit.
6. UI: the scope menu, Viewed in the rail, `v`, `}` and `{`. Commit.
7. `perch-lsp` and backend: references. UI: the hover panel entry, the
   bottom panel, `Shift-F12`. Commit.
8. Backend and UI: the split layout. Commit.
9. Knowledge: local threads archived; the skill text. Commit.
10. README: the item view section replaces the review and issue
    sections; the keys table. Commit.

Steps 1, 2 and 3 are independent. Steps 7 and 8 are independent of each
other and of 6.

## Acceptance

- Open a merged pull request with reviews and line comments. Conversation
  shows the description first, the state pill says Merged, and the strip
  names the reviewer with a check. Comments, reviews and the merged event
  are in time order. A review's line comment shows its lines tinted;
  click the path and Files opens on that file at that line.
- Select lines on Files and ask. The thread appears in the column and, on
  Conversation, as a dashed card with the same lines, the model, and a
  Local chip. Follow up from the card; the column shows the reply. Follow
  up from the column; the card shows it.
- Hover a GitHub comment, click Ask about this. The composer opens with
  the comment quoted. Send. The resulting card is dashed, says Local, and
  the quote is in the question.
- Open a branch item with no pull request. One Conversation with the
  empty message and the composer; Files is the diff. Ask; the card
  appears.
- Open an issue. One tab. The view is what it was, with the labels,
  assignees and In Perch chips in the strip.
- Open the scope menu on a branch with three commits. Pick one. The
  button names it, the rail lists only its files, the tints match `git
  show`, the header churn matches, and the tab count follows. Pick
  Branch; everything returns.
- Edit a file in the checkout from a terminal. Uncommitted is no longer
  greyed. Pick it; the rail lists that file, added lines tinted against
  `HEAD`. Stage it; Staged shows it and Unstaged is greyed.
- Check Viewed on a file. The row goes muted, the count says 1 of N,
  and `}` skips it. Pick another scope where the file is the same blob;
  still viewed. Commit an edit to it; unviewed. Quit and reopen; the
  checks held.
- Open a pull request opened yesterday whose head moved. Since you last
  looked is enabled and lists the new commits' files. Reopen without a
  push; it says No new commits.
- Toggle Split. Base on the left, head on the right, aligned, one
  scrollbar. Select lines on the right and ask; the anchor is the head's
  lines. Quit and reopen; Split held.
- Click a symbol, Find references. The panel lists them grouped by file;
  click one and the editor opens there. `Escape` closes the panel.
- A pull request whose latest run failed shows a red pill; click opens
  the run. One with no runs fetched shows no pill.
- Go offline. Open a pull request opened before. Conversation paints
  from the cached discussion, the scope menu from git, and the checks
  pill is whatever was last known.
- `perch --timeline` and `perch --scope` print what the window shows.
- `cargo test` green, `cargo clippy` clean.

## Things the implementer should not do

- Do not give a GitHub card a reply box, a resolve button, or an emoji
  row. It has a permalink and Ask about this.
- Do not let a thread card look like a comment card. The dashed border,
  the canvas background and the Local chip are all required, not a
  choice among them.
- Do not render the thread twice. One set of views, a `ThreadFace`.
- Do not add a Commits tab, or a commits pane in the rail. The menu is
  the list.
- Do not let the scope leak past the diff. Everything downstream reads a
  changed list and a pair of sides. If a view needs to know which scope
  is set, the changed list is missing a field.
- Do not key Viewed on the scope, the commit or the head sha. It is the
  blob. A file viewed is a content viewed.
- Do not clear Viewed on push. Rows are never deleted by a fetch; a
  changed file has a new blob and no row.
- Do not fetch the timeline endpoint. Events come from the pull request
  and review payloads. If an event is wanted that they do not carry, it
  waits.
- Do not store a GitHub comment id on a local thread. The quote is the
  link. A thread that pointed at a comment would need to know when the
  comment was edited or deleted, and nothing here handles that.
- Do not compute the split alignment in the view. The backend owns the
  diff; the view draws rows.
- Do not let the left side of a split take a selection or an anchor.
  Anchors are on the head, and `Side::Left` in the model is for GitHub's
  comments on removed lines, not for new ones.
- Do not restart rust-analyzer on a scope change. It watches the disk;
  the notice says so when the head side is not the disk.
- Do not poll checks. One request on open, one on a CI notification.
- Do not add a Checks tab, and do not draw a grey pill for "not fetched".
  Absent means unknown.
- Do not change the rail's two modes. Changed files and the whole tree
  are what they were; the scope only decides which files count as
  changed.
- Do not migrate. Bump the version, recreate. Say in the commit message
  that this is the last one.
- Do not write call hierarchy. References is the deliverable.
