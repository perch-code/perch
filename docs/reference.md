# Perch reference

Perch is a native macOS coding and review tool. It combines agent-driven
worktrees, code browsing and LSP support, GitHub reviews and issues, and local
human/agent conversations in a compact Rust application built with Floem.

## GitHub inbox

Perch builds its inbox from GitHub's REST notifications list. It preserves the
API's order and unread state, and caches the latest response locally so the
sidebar can paint immediately. Perch requests `all=true` so read threads remain
visible alongside unread ones. If the list cannot be written to the local
database, the sidebar still shows it, leaving out what is Done or snoozed as
the marks on disk say.

Polling runs in the background, respects GitHub's `X-Poll-Interval`, and
paginates the full response. Perch performs an unconditional reconciliation at
startup and on the first scheduled poll after 15 minutes. Polls between
reconciliations ask for the first page only if its `ETag` has changed, and read
the whole list again when it has. New activity always changes the first page. A
thread past the first fifty does not when it is read or marked Done on
github.com, or when only its time moves (your own comment or review, a label),
and the reconciliation picks that up. Without a token, each poll looks for
one again, so a `gh auth login` after launch is picked up.

A poll never moves what is on screen. When it would add, remove, or reorder
the rows the sidebar shows, the rows stay as they are and a pill over the list
says how many are new, with Refresh to show them. Changes inside a row, such
as read state, go straight in, and anything you do in the sidebar shows the
latest list. When a notification arrives about the open pull request or
issue, a pill over the pane offers to fetch it. A CI notification for the open
pull request updates its checks directly, as long as the head on screen is
still GitHub's.

- Viewing a pull request or an issue marks its GitHub thread read, from
  whichever row it was opened: the notification's, or the item's in All or
  Authored. Read threads remain in the inbox. While the thread is unread, the
  item's row is bold with the unread dot, as the notification's row is.
  GitHub's list can go on reporting a thread unread for a while after it has
  taken the request, so a read made in Perch holds for that revision of the
  thread until newer activity arrives or it is marked unread; a thread marked
  unread on github.com for the same revision is not seen.
- Opening an unread notification, from its row or from the item's in
  Worktrees or Browse, goes to what it is about: Conversation
  scrolls to the newest comment, review, reply or event no later than the
  notification, and the card flashes. A resolved thread with the reply is
  opened. The cached discussion paints first, so the page waits up to a
  few seconds for GitHub's answer to bring the new activity, then settles
  for the newest card it has. Scrolling first leaves the page where it
  is, and a read notification opens where the page was left.
- **Mark Unread**, on the notification's row or the item's, puts an unread
  reminder on the thread in Perch. Viewing it in Perch clears the reminder,
  and newer activity GitHub reports unread takes its place. A later update
  GitHub reports read leaves it: your own comment or review moves the thread
  too, and so does activity you have already read on github.com. GitHub's
  public API has no mark-unread operation, so this does not change
  github.com.
- **Done** marks the thread done on GitHub and remembers that update in
  Perch, so GitHub's `all=true` history cannot put it back. Newer unread
  activity clears the local mark and lets the thread return. A later update
  GitHub reports read does not: your own comment, review or merge moves the
  thread too, and so does activity you have already read on github.com.
  Done is for the update the row shows. Perch reads the thread from GitHub
  just before marking it, one extra request per thread, and a thread with
  newer unread activity is not marked Done. One the pill over the list was
  holding back stays, with what arrived, and so does the pane showing it; one
  that arrived since the last poll comes back behind the pill. A notice says
  so either way. A comment that lands between that read and the Done still
  goes with it: GitHub's Done takes whatever the thread holds.
- **Unsubscribe** ignores future activity on the thread, marks its current
  notification done, and remembers it the same way. Whatever has arrived on
  the thread goes with it.
- A thread whose pull request you merged or closed, or whose issue you
  closed, with nothing on it since, is marked Done on its own, on GitHub
  too. A merge or close by anyone else stays in the inbox, read or not, so
  you see it. Perch waits a minute after the close first: GitHub sends the close's
  own notification some twenty seconds late, and a Done before it would see
  the thread come back. A comment more than a minute after the close keeps
  the thread, or returns it; one inside that minute is taken for the close's
  own notification. The thread is read first here too, and one with newer unread
  activity waits for the poll that brings it.
- **View › Done**, in the Inbox's filter menu, lists the threads marked Done
  in Perch that GitHub still lists. **Move to Inbox**, on the row or its menu,
  drops the local mark and puts the thread back. GitHub's public API has no
  way to undo Done, so github.com keeps it under Done. A closed thread you
  move back to the inbox stays; only a later merge or close, with nothing
  after it, marks it Done again.
- **Snooze**, the clock on a row or the Snooze submenu in its menu, hides the
  thread until later today (6 PM), tomorrow, later this week, this weekend or
  next week (8 AM), as Gmail offers them. It comes back sooner if newer
  activity arrives that GitHub reports unread. When the time comes it
  returns to its place in the inbox, as read or unread as it was snoozed. **View › Snoozed** lists the
  snoozed threads with when each returns, and **Move to Inbox** ends a snooze
  early. GitHub has no snooze, so the mark lives only in Perch and
  github.com still lists the thread.
- **State › Merged or Closed** narrows the inbox to threads whose pull
  request has been merged or closed, or whose issue has been closed. A thread
  whose subject Perch has not looked up yet counts as open, and so do kinds
  with no state, such as releases and CI runs.
- **Mark N Done**, at the foot of the filter menu, marks every thread the
  inbox is showing Done, one GitHub request after another, each read first,
  so a thread with new unread activity is left in the inbox, and reports any
  failures in a single notice. Filtering to State › Merged or Closed first
  clears the finished pull requests in one go. On View › Done and View ›
  Snoozed the same item reads **Move N to Inbox**.
- Pull requests and issues open inside Perch. Other notification types remain
  first-class inbox rows and open on GitHub.

### The sidebar's filter menus

Each tab has one filter menu, laid out the same way: View, then what the
list is narrowed to, then Sort By, then display settings and actions. A
section is left out only where it cannot apply. The button lights up while
the tab's list differs from its defaults, and the search box names the view
it searches.

- **Inbox**: View (Inbox, Snoozed, Done; then Review requested, Yours,
  Assigned, Mentioned, All, from GitHub's reason, Yours also taking what you
  opened), Repository, State, Sort By (Newest or Oldest first),
  Unread Only, Mark N Done.
- **Worktrees**: View (Worktrees, or Recent: everything opened, worktree or
  not), Repository, State, Sort By (Last activity, Last opened, Created; each
  view keeps its own), Unread Only.
- **Browse**: View (Review requested, Yours, Assigned, Mentioned, All),
  Repository, State, Sort By (Last activity or Created, sorted by GitHub's
  search). Repository offers every repository on GitHub
  that config, Worktrees or Inbox knows of. All searches the repository
  picked, or every declared repository on GitHub.

Repository is shared by all three tabs. State is Any,
Open, or Merged or Closed everywhere; it defaults to Any on Inbox and
Worktrees and to Open on Browse.

GitHub's public REST notification payload does not expose whether a thread is
currently in the web Inbox or Done, and `all=true` can include Done history.
Perch can therefore keep its own Done actions stable, but cannot reliably
mirror Done or Restore actions performed on github.com.

These actions affect only the notification or its local reminder. They do not
archive or delete a Perch item, worktree, or local conversation.

### Finished runs on your own work

A worktree with no pull request has no one on GitHub to notify you, so Perch
does. When an Ask or Work run, or a worktree review, ends on it, the item gets
a row in the inbox: its glyph and title as Worktrees draws them, then the
repository, the branch, and what happened: `Claude · Work · 3 files`,
`Codex · Ask`, `Review · 2 findings`, in the failure colour when it failed. A
run you stopped or cancelled, or that ended because Perch quit, doesn't count,
and neither does one on the item open in a window that's in front: you watched
it. A Claude Code or Codex session you started yourself, outside Perch, isn't
seen.

The item has one row, for the last run that ended there. It fits in among
GitHub's rows by time, and waits behind the pill as a poll does. It is unread
until you open the item, from any row, and the item's row in Worktrees is bold
with the dot until then too. Opening it opens the item. **Mark Unread** puts
the dot back. **Done** puts it away until the next run ends there; it never
archives the item or touches its worktree. Local rows are under View › Inbox
with Yours or All, not under GitHub's own reasons, and have no Snooze.

Once the item has a pull request, GitHub's notifications are its signal: its
row goes, and a run that ends there makes none, only the notice a run leaves
as it finishes.

Perch does not crawl a repository's open items or history. A pull request or
issue is fetched when a notification is opened or when the user opens it by
repository and number. Its current data and discussion may be cached locally;
closed history is never backfilled.

## Conversation and Code

An item has two modes, the pills at the centre of its bar, as Codex has
Summary and Code. **Conversation** is the item's summary, its timeline and its
threads, with a composer at the foot. **Code** is the worktree: the open
files as tabs over the editor, and the file tree in the drawer on the right
(see [The file tree](#the-file-tree)). When the bar's controls take more
than half of it, the pills give way to them rather than sit at the centre.

An issue's Code is the repository's default branch, read-only: the home
checkout questions about its issues run in (see [Agents](#agents)).
Its pills come once that checkout exists, which the first issue opened in a
repository makes. The top of the file tree names the branch and its commit
in place of the scope, says when Perch last fetched it, and has a refresh
that fetches it again; opening an issue fetches it too, at most once a
minute per repository. Files the issue names open there, and Cmd-P finds
any file on the branch. There is no change to fold or split, so Code's
tabs have Word wrap and the tree alone. Since every issue of a repository reads one checkout, a fetch
moves the files under all of them, and open files reload. To change the
code, Start work: see [Work](#work).

Each item comes back on the mode it was left in, with its tabs, until Perch
quits; it opens on Conversation the first time. Code with no file open says
so, or says there are no changes, and an item's first visit to Code opens the
tree beside it. Closing the last file's tab leaves Code showing. Conversation
cannot be closed.

| Action | Key |
|---|---|
| Conversation, Code | `Cmd-Shift-1`, `Cmd-Shift-2` |
| Next, previous file tab | `Cmd-}`, `Cmd-{` |
| Close the file | `Cmd-W` |
| The file tree | `Cmd-Shift-E` |
| Agents | `Cmd-Shift-A` |

These are the default keys; [Keyboard shortcuts](#keyboard-shortcuts) has
every key and how to change it.

## Timeline

Conversation is the item's description, what was said about it, and what was
done to it, in one column in time order. The doing comes from GitHub's own
timeline: commits, labels added and removed, assignments, review requests and
dismissals, renames, milestones, force pushes, references from other issues
and pull requests, and the opening, merge or close. Each is one line saying
who did it and when, with a label in its own colours, a commit by its short
sha and subject, and a reference by where it points. A commit is signed with
the name git recorded, since it has no account behind it.

What Perch already draws as a card — a comment, a review, a line comment — is
not repeated as a line, and what GitHub sends but does not itself show, such
as `mentioned` and `subscribed`, is left out. A kind Perch has no words for is
dropped rather than guessed at. Ten pages are read for one item, a hundred
lines to a page.

## Reactions

Each GitHub entry on Conversation shows the reactions on it under its text:
the description, every comment, and every review summary and line comment. A
pill is one reaction and its count, blue when yours is among them. Hovering
over a pill says who left it, as GitHub does: "You, octocat, and 3 more
reacted with heart emoji", naming up to ten. Clicking a pill adds or removes
yours. The face at the right of an entry's byline, and
the one after its pills, open GitHub's picker: all eight in a row, yours
washed blue, and picking one of yours takes it back. A reaction is shown as
soon as it is clicked and put back if GitHub refuses it.

Reactions are read with the discussion through GitHub's GraphQL API, a
hundred entries to a request, and left the same way. An entry cached by an
older build of Perch has no reactions until it is fetched again.

## Posting

A composer's box opens five lines tall, the size GitHub gives a comment, and
grows with what is typed. The grip at the foot of the box drags it taller or
shorter, between two lines and about twenty-four; a box that has been dragged
keeps that height whatever is typed into it, and every composer opens at it
until Perch is restarted.

Typing `@` in a composer, or in the review summary under **Review**, puts a list under
it, as GitHub's comment box does. People come first: the item's author and
whoever has written on it, then anyone else in the repository GitHub finds
for the letters typed, never you. The worktree's files follow, ranked as
`Cmd-P` ranks them. The arrows or `Ctrl-N` and `Ctrl-P` move the pick, `Enter`
or `Tab` takes it, a click takes a row, and `Escape` puts the list away until
the next `@`. A person goes in as `@login`. A file goes to an agent as
`@path`, as Claude Code and Codex take one, and to GitHub as `` `path` ``,
since `@` there mentions someone and a code span links to the file on Code.

Every composer in Perch has a destination: Claude, Codex, or GitHub, picked
from the same menu as the model. What goes to an agent stays in Perch. What
goes to GitHub goes one of two ways.

On a line of a file, or under a thread that is on GitHub, a message to GitHub
is a draft: it is kept in Perch's cache the moment it is sent, drawn with a
`Draft` pill, marked in the gutter in GitHub's blue, and can be edited in
place or deleted on hover. Drafts survive a restart and being offline, and
github.com does not show them. **Review**, in the bar, counts them on its
button, lists them above the summary in its popover, and posts them with the
verdict: a comment on the
head rides in the review, a comment written against an older commit is posted
against that commit, and a reply is posted under its thread. A `Comment`
review with drafts needs no summary. A draft that GitHub refuses, or that
cannot be matched to what GitHub returns, stays a draft and the form says so.
GitHub's API takes a review comment only on the lines of the pull request's
diff, a changed line or one of the three around it, though github.com takes
one on any line, and one comment it refuses fails the whole review. So a
comment on the head that is not inside one hunk of the diff, first line and
last, is kept out of the review: the rest posts, and it stays a draft, named
by its lines on the form.

A composer on lines has **Suggest**, which puts a `suggestion` fence in the
box holding the lines as they are, to edit into what they should read. On a
worktree with no pull request there is no one on GitHub to accept it, so a
message that is a suggestion and nothing else goes straight into the file
instead of to the agent, as Commit suggestion does on github.com. It goes in
only while the file still reads those lines; otherwise the composer says they
changed and keeps the text. The edit is uncommitted, so under a scope that
shows a commit, such as Branch, the view moves to Uncommitted to show it. A
suggestion with words beside it goes to the agent as any message does.

On an open pull request, a suggestion in a comment on GitHub has **Apply
suggestion** under it. It opens a popover with the commit message, `Update
<path>` to start, and an optional description; **Commit changes** (or Enter in
the message) commits the change on the pull request's head branch through
GitHub, as github.com does, and resolves the thread where GitHub lets you. The
file is read at the head GitHub has then, and the commit goes only while the
lines still read as the suggestion was written against and the branch has not
moved; otherwise the popover says why and stays. Your worktree is not touched:
it is behind by that commit until you pull. The button is off on a resolved
thread, offline, or when the suggestion changes nothing, and absent on an
outdated thread, a draft, or a closed pull request.

At the foot of Conversation, a message to GitHub is an issue comment and
posts at once, after one confirmation naming the pull request. Beside
**Comment**, **Close with comment** posts it and closes the pull request, or
reads **Close pull request** while the box is empty. It is there on an open
pull request you opened or can write to, and asks once too. It is a split
button, as on github.com: its arrow opens a menu of the ways to close. On a
pull request those are **Close pull request** and **Close as issue…**. On an
open issue you opened or can triage, the button reads **Close issue**, and the
menu offers **Close as completed**, **Close as not planned** and **Close as
duplicate**, the reason GitHub records for the close. A pick changes what the
button says and does; it stays picked until the item changes. Close as
duplicate searches the repository's issues, as a `#` does, and picking the
original goes straight to the confirmation, which names it. GitHub has no
threads under a comment, a review's summary or the description, so each offers
**Quote reply** under the pointer, which opens that composer with the text
quoted as github.com quotes it, below anything already typed there for GitHub.
With words of it selected, it quotes just those.

**Close as issue…**, in that menu, is for a pull request that found a real bug
but isn't worth merging. It unfolds a form above the composer: the issue's
title and body, filled from the pull request with a closing `Found in #12.`
that links the two, and the comment the pull request gets, where `{issue}`
becomes the new issue's number. **Write with Claude**, or Codex, has the agent
read the pull request's checkout and diff under Ask's rules and describe the
bug as it stands on the base branch, in its own words rather than the pull
request's. The answer fills the title and body for you to edit, asking first
if you have typed there. Nothing goes to GitHub until **Open issue and close**,
which opens the issue, comments and closes. Once the issue is open the form is
spent, so a comment or close GitHub refuses after it is told as a notice, not
retried.

GitHub's review threads are drawn beside Perch's own with the same cards, in
both Code and Conversation, so one thread can hold a reviewer's comment, your
reply to them, and an agent's private take on both. A thread looks as it does
on github.com: on Conversation, a box headed by its file with the lines it is
on under the header; in Code, a box under those lines headed "Comment on line
R12", or L12 on a line of the old file. The split layout draws each box on its
side: the old file's on the left, beside fillers on the right, and the new
file's on the right. Each message sits under its author's face and name, with **Author** by
whoever opened the pull request, and the thread ends in a **Follow up** box and
a foot with **Resolve**. A reviewer's comment offers **Reply** and **Ask about
this** under the pointer. In such a thread, what an agent said and what you
asked it are labelled **Perch only**.
An agent's answer never posts on its own: **Post to GitHub** under the pointer
puts its text in a composer for you to edit and send, and nothing records
where it came from. It goes the same two ways as anything else: a draft under
a thread with lines or a place on GitHub, an issue comment under one of
Perch's own, such as a question asked at the foot of Conversation. A review's
findings on an open pull request arrive as drafts already (see Worktree
reviews), and post only with the review. **Quote reply** on an answer quotes
it, or the words of it you selected, into the thread's Follow up box, still
bound for the agent.

A thread ends in **Resolve**, or **Unresolve** once it is resolved. On a
thread that is on GitHub the button is there wherever GitHub says you may
use it, and the click changes github.com at once, with no confirmation, as
the button there does. The thread shows the change before GitHub answers
and goes back if GitHub refuses. On a thread of Perch's own, a question to
an agent and its answers, the button is always there and the change stays
in Perch. A review finding has **Dismiss** and **Reopen** instead, and
once published it is on GitHub and has **Resolve** too.
Resolved, a thread folds, in Code to one row and on Conversation to its
header, as one resolved on github.com arrives; **Show resolved** opens it
again and **Hide resolved** folds it. Conversation and Code each keep their
own, as github.com's tabs do, and the chevron on a thread's header folds any
thread.

## Authentication

Perch calls GitHub's API directly. It resolves credentials in this order:

1. `GITHUB_TOKEN`
2. `GH_TOKEN`
3. `gh auth token`
4. `token` in `~/.perch/config.toml`

Using `gh auth login` is the simplest setup for the desktop app because a GUI
launch does not normally inherit shell environment variables. The credential
must be allowed to read and update notifications and to read the repositories
you open. Merging a pull request also requires `Contents: write` permission on
its repository, as does pushing a branch from Perch's own clone; opening a pull
request requires `Pull requests: write`. Commenting on, approving, or requesting changes on a pull
request requires `Pull requests: write` permission, as do resolving a thread
and reacting to a pull request or anything said on it; reacting on an issue
requires `Issues: write`, as do Close as issue and closing an issue.

## Configuration

Perch reads `~/.perch/config.toml` (or `$PERCH_HOME/config.toml`). A repository
can have a GitHub identity, a local root, or both:

```toml
poll_interval_secs = 60

[[repo]]
github = "slatedb/slatedb"
name = "slatedb"
root = "~/Code/slatedb"
```

`name` is only the sidebar label. `root` lets Perch discover existing local
branches and worktrees. Neither setting causes a repository-wide GitHub crawl.
A worktree with no pull request is yours, so it is in **Authored**, whether Perch
made it or found it under a root; the base branch never becomes a row. Once
someone else's pull request attaches to it, the row goes by that author.
Perch never removes a worktree it found under a root, so deleting its row
lasts only until the next scan finds it again. A branch renamed with
`git branch -m` keeps its row, with its threads, runs and name, and a hidden
row stays hidden; the scan knows it by the rename in the branch's reflog.
That is what T3 Code does to a thread's `t3/<hex>` branch once the thread
has a title. A branch switched rather than renamed puts the old row away as
before, even when the old branch is deleted before the next scan, and so
does a rename onto a branch another row already has, or of a branch with a
pull request, which GitHub still has on the old name.
**Hide** in the row's menu
keeps it out of Worktrees without touching it; it's listed under Recent,
where **Unhide** brings it back.
A repository with a root and no GitHub remote is `local/<name>`: it has no pull
requests or issues, and everything else works.

New work (see [Starting work](#starting-work)) reads two more tables:

```toml
[work]
branch_prefix = "chris/"       # before a branch name Perch fills in
approvals = "auto"             # or "ask"; see Approvals

[[repo]]
root = "~/Code/perch"

[repo.work]                    # what a Work run here may reach
approvals = "never"            # over [work]'s, for this repository only
write = ["~/.cargo/registry", "~/.cargo/git", "~/.cargo/.package-cache"]
network = ["index.crates.io", "static.crates.io"]
trust = false                  # true loads the repository's own agent config
```

Both lists are empty unless given. A `write` entry is a place the agent can
leave something for the next program that reads it, which is why the example
names cargo's caches and not `~/.cargo`: `~/.cargo/config.toml` and
`~/.cargo/bin` would let a run change what `cargo` does when you run it. An
entry that is or holds the home directory or `~/.perch`, or that is in or
holds a root's `.git`, is left out, with a notice at startup.

A language server is kept running for a while after its item is left, so
going back to it doesn't wait for the workspace to load again: two at most,
ten minutes each. On a large workspace each can hold several gigabytes, so
`[lsp]` caps what every server uses between them:

```toml
[lsp]
max_memory_mb = 8192           # the default
```

Past it, the kept servers are stopped, oldest first. The ones on show never
are, so their own total can go over.

`[display]` sets how big Perch draws, read when Perch starts:

```toml
[display]
zoom = 1.0                     # the default; 0.75 to 2.0
code_font_size = 12            # the default; 8 to 32
```

`zoom` scales the whole window: text, code, icons and spacing. On a large
monitor, 1.25 is the same layout a quarter bigger. `code_font_size` is the
code in Code, sticky headers, hovers and references, before the zoom; the
line height follows it. The UI's own text, and code inside a comment or an
answer, change only with `zoom`.

`[ui]` sets the look:

```toml
[ui]
appearance = "dark"            # "light", "dark" or "system", the default
code_font = "Fira Code"        # off macOS only
```

Perch is light or dark as the system is unless `appearance` says otherwise.
On Linux under X11 the system's setting doesn't reach Perch, so there it is
light. A new `appearance` applies at once, but going back to `"system"`
from light or dark takes the system's appearance as it is then, and Perch
follows the system's later switches only after a restart. On macOS code is
set in SF Mono. Elsewhere it is `code_font`, read when Perch starts, else
the first installed of JetBrains Mono, DejaVu Sans Mono, Noto Sans Mono,
Liberation Mono, Adwaita Mono and Droid Sans Mono.

rust-analyzer and jdtls run a checkout's own code as they load it:
rust-analyzer its build scripts and proc macros, following the checkout's
`.cargo/config.toml` and `rust-toolchain.toml`, and jdtls its Gradle or Maven
build. Perch starts them by itself only on your own work in a repository in
your config: a branch, the default branch an issue reads, and a pull request
you opened whose head is in that repository or your fork of it. A branch in
your root counts as your own, so one you check out there from
someone else's fork, as `gh pr checkout` does, starts them too. On anything
else, such as someone else's pull request, a branch you cut from one, or a
repository not in your config, they wait, and a toast names the item with
**Trust and start**. Perch remembers that answer for the item, commits pushed
to it later included; deleting the item forgets it. Under the toast's line
each server held has a box, **Always trust rust-analyzer in
slatedb/slatedb**: ticked when you press Trust and start, that server starts
on every item in the repository from then on, anyone's pull requests
included, and no toast asks for it there again. Leave it empty and the next
pull request or worktree there asks again. While they wait, the Worktree row
on Conversation has a warning, and its menu has **Trust and Start** and
**Always Trust** for each server held, once the toast is closed. On any item
in a repository, the menu has **Stop Trusting** for each server trusted on
all of it, which stops that server wherever nothing else trusts the code and
asks again. After upgrading, a branch you had already cut from a pull
request, your own included, asks once. gopls and typescript-language-server
start either way. To start every server on everything in a repository,
anyone's pull requests included:

```toml
[[repo]]
github = "slatedb/slatedb"

[repo.lsp]
trust = true                   # false, the default, asks as above
```

Perch's database, avatars, managed clones, worktrees, and agent state live
under `~/.perch`. The database also holds local items, conversations, and
review state, so it is not disposable. GitHub notifications and fetched
remote data within it are refreshed on demand.

Schema upgrades use preserving, transactional migrations: local discussions,
agent attempts, session IDs, and activity history survive. Perch refuses an
unsupported or newer schema without deleting it, and a failed migration leaves
the original database in place.

## Settings

`Cmd-,` (`Ctrl-,` off macOS), **Settings…** in the Perch menu, or the gear
at the foot of the sidebar opens Settings over the window: General,
Environment, Appearance, Keyboard shortcuts, Agents, Work, Repositories and Language
servers. The search box finds a setting in any section. `Escape` clears the
search, then closes Settings, as do `Cmd-[` and `Cmd-W`, or the keys
`back` and `close` are on. Keyboard shortcuts has a row for each action a
key runs, where its keys can be changed: see
[Keyboard shortcuts](#keyboard-shortcuts).

The environment choice under **This window** lasts for that window.
Preferences are saved in `config.toml`. Each preference writes its own key and leaves the rest of the file as it
was, comments included. The arrow beside a setting's title shows the file
sets it; clicking it takes the key out, back to the default. A text field
writes on `Enter` or when you leave it, and `Escape` puts back what the file
has. Repositories has a card for each `[[repo]]`, with **Remove
repository** and, at the end, **Add repository…**.

Perch looks at the file every two seconds, so an edit you make by hand is
taken up while it runs. A file that doesn't parse changes nothing: Settings
says why and turns its controls off, and Perch goes on with what it read
last until the file is fixed. **Open in editor** opens it.

Most settings apply at once. A new executable or models list makes Perch
look for the agents again. Lowering **Runs at once** lets the runs going
finish and holds new ones until there is room. Turning off **Start language
servers without asking** sends servers on code no longer trusted back to
asking. Zoom, code font size and the code font apply after a restart;
Settings marks the ones waiting and offers to restart Perch.

## Keyboard shortcuts

Every key below can be changed, in `[keybindings]` in `config.toml` or in
Settings' Keyboard shortcuts section. Each action has an id; give it a key
as a string, several as a list, or none as `""` or `[]`. Keys you give an
action take the place of its defaults, so a list that leaves a default out
drops it.

```toml
[keybindings]
go_to_file = "super+t"          # Go to File on Cmd-T, and no longer on Cmd-P
pin = ""                        # no key; [] does the same
refresh = ["super+r", "f5"]     # two keys for one action
toggle_sidebar = 'super+\'     # a backslash in single quotes, or "super+\\"
```

A key is written as its modifiers, then the key, joined by `+`, in any
case:

- Modifiers: `super` (also `cmd`, `command` or `meta`), `ctrl` (or
  `control`), `alt` (or `option`) and `shift`. `super` is the command key:
  Cmd on macOS, Ctrl elsewhere, so off macOS `ctrl+p` and `super+p` are the
  same key.
- Keys: a letter, a digit, punctuation as a US keyboard has it
  (`` [ ] \ / , . ; ' = - ` ``), `enter`, `escape`, `tab`, `space`,
  `backspace`, `delete`, `up`, `down`, `left`, `right`, `pageup`,
  `pagedown`, `home`, `end`, `f1` to `f20`, or a key's code as floem names
  it, such as `KeyH` or `NumpadEnter`. Keys are physical: `shift+]` is the
  key a US keyboard writes `}` with, wherever your layout puts it.
- A backslash is an escape in a TOML string in double quotes, so write
  `super+\` as `"super+\\"`, or as `'super+\'` in single quotes. `"super+\"`
  leaves the string open, and Perch can't read any of `config.toml` then.

The modifiers must be the ones held, no more: `super+g` doesn't run on
Cmd-Shift-G. An id Perch doesn't have, or a key it can't read, is left out
with a notice, and the rest of the table still applies.

A key with no Cmd, Ctrl or Alt that types something, a letter, a digit,
punctuation or the space bar, Shift or not, is a bare key. A bare key never
runs an action while the keyboard is in a box: the sidebar's search, a
composer, Review's form, a find bar or any other text field. There it types,
so `/` searches the sidebar from anywhere else and is a `/` in a comment.

Two actions heard in the same place shouldn't share a key: only one of them
would run. Settings says when they do. The composer's keys are heard only in
a composer, so they can share a key with most of the window's: `super+g`
suggests a change in a comment and finds the next match everywhere else. Not
with the window keys that work from a composer too, such as New Work, Close,
Go to File or Toggle Files: the window hears those first, so the composer's
action would never run.

On macOS, New Window, Open… and Settings… in the Perch menu show their
action's first key, unless it is bare; the other keys work in the window.
Menus that show keys, such as a worktree's menu in an item's details and the
right-click menu in the code, show the first key of the action they do. A
button with a key says it in its tip.

These stay as they are: `Escape`, `Enter`, the arrows and `Tab` inside
menus, Go to File, pickers, dialogs and the find bar; the text keys of boxes
and the editor, `Cmd-C`, `Cmd-A` and the like; `Cmd-Enter`, which sends or
submits; `Cmd-click` on a symbol; the mouse's back and forward buttons; the
single letters of an open item menu; and the keys macOS keeps, `Cmd-Q`,
`Cmd-H`, `Cmd-Opt-H`, `Cmd-M`, `` Cmd-` `` and `Cmd-Tab`. Settings won't take
these macOS keys for an action. An action given `Cmd-Q`, `Cmd-H`,
`Cmd-Opt-H` or `Cmd-Tab` by hand never runs, as macOS has them first, and
Settings says so on its row.

Settings' Keyboard shortcuts section has a row for each action, in the
groups below. Click its keys, or press `Enter` or `Space` on them, then
press the new key: it replaces every key the action had. `Escape` stops
without a change, and a key macOS keeps is refused. macOS acts on `Cmd-Q`,
`Cmd-H`, `Cmd-Opt-H` and `Cmd-Tab` before Perch hears them, so pressing one
there does what it always does. The × beside the keys
takes them all away, and the reset arrow puts the defaults back. Several
keys for one action are written in the file.

Hold Command on macOS or Ctrl on Linux to see the current shortcuts under
the sidebar tabs, sidebar toggle, New Work, and the Conversation and Code tabs.
The Inbox, Worktrees, and Browse defaults yield to any shortcut you assign
in Settings or in `[keybindings]`.

Each action's id, its default keys and what it does, grouped as Settings
groups them:

**General**

| Action | Default | What it does |
|---|---|---|
| `new_work` | `super+n` | Opens New Work, to start an agent on a new branch |
| `new_window` | `super+shift+n` | Opens another window |
| `open` | `super+o` | Opens a pull request or issue from its link or number |
| `settings` | `super+,` | Opens Settings |
| `close` | `super+w` | Closes the open file, else the open item, else the window; closes Settings too |
| `back` | `super+[` | Closes the open item or New Work, back to the sidebar; closes Settings too |
| `history_back` | `ctrl+-` | Goes back to where you were |
| `history_forward` | `ctrl+shift+-` | Goes forward again after going back |
| `toggle_agents` | `super+shift+a` | Opens or shuts the agents running across Perch |
| `send_feedback` | none | Opens or shuts Send Feedback |

**Sidebar**

| Action | Default | What it does |
|---|---|---|
| `toggle_sidebar` | `super+\` | Shows or hides the sidebar |
| `focus_search` | `super+1`, `/` | Puts the keyboard in the sidebar's search, opening the sidebar if it is shut |
| `inbox` | `super+2` | Shows the Inbox, opening the sidebar if it is shut |
| `worktrees` | `super+3` | Shows Worktrees, opening the sidebar if it is shut |
| `browse` | `super+4` | Shows Browse, opening the sidebar if it is shut |
| `next_item` | `j` | Opens the row after the open one in the sidebar's list, or the first; a notification that opens in the browser is passed over |
| `previous_item` | `k` | Opens the row before the open one |
| `mark_done` | `e` | Marks the Inbox row under the pointer done, else the open item's row |
| `mark_unread` | `shift+u` | Marks the Inbox row under the pointer unread, else the open item |
| `pin` | `p` | Pins or unpins the row under the pointer, else the open one |

**Item**

| Action | Default | What it does |
|---|---|---|
| `show_conversation` | `super+shift+1` | Shows the open item's Conversation |
| `show_code` | `super+shift+2` | Shows the open item's Code |
| `next_tab` | `super+shift+]` | Goes to the next file tab, `Cmd-}` on a US keyboard |
| `previous_tab` | `super+shift+[` | Goes to the file tab before, `Cmd-{` |
| `refresh` | `super+r` | Fetches the open item from GitHub again |
| `open_on_github` | `super+shift+o` | Opens the row under the pointer, else the open item, in the browser |
| `copy_link` | `super+shift+c` | Copies the open item's link |
| `copy_branch_name` | none | Copies the open item's branch name |
| `start_work` | none | Starts work on the open item |
| `toggle_review` | `super+shift+r` | Opens or shuts the Review box, where the bar has Review |
| `toggle_merge` | `super+shift+m` | Opens or shuts the Merge box, where the bar has Merge |

**Conversation & Code**

| Action | Default | What it does |
|---|---|---|
| `comment` | `a` | Comments on the selected lines on Code, or on the item as a whole on Conversation |
| `page_up` | `pageup` | Scrolls up a page |
| `page_down` | `pagedown` | Scrolls down a page |
| `scroll_to_top` | `super+up` | Scrolls to the top |
| `scroll_to_end` | `super+down` | Scrolls to the end |

**Code**

| Action | Default | What it does |
|---|---|---|
| `go_to_file` | `super+p` | Opens Go to File; again while it is up, picks the next file |
| `toggle_files` | `super+shift+e` | Opens or shuts the file tree |
| `next_change` | `]` | Goes to the next change in the file |
| `previous_change` | `[` | Goes to the change before |
| `toggle_viewed` | `v` | Marks the open file viewed, or not viewed |
| `next_unviewed` | `shift+]` | Goes to the next file not yet viewed |
| `previous_unviewed` | `shift+[` | Goes to the file before not yet viewed |
| `toggle_wrap` | none | Turns Word wrap on or off |
| `go_to_definition` | `f12` | Goes to where the symbol at the caret is defined |
| `go_to_type_definition` | none | Goes to where the symbol's type is defined |
| `go_to_implementations` | `super+f12` | Lists what implements the symbol |
| `find_references` | `shift+f12` | Lists where the symbol is used |
| `call_hierarchy` | `alt+shift+h` | Shows what calls the function, and what it calls |

**Find**

| Action | Default | What it does |
|---|---|---|
| `find` | `super+f` | Opens the find bar on the code or Conversation |
| `find_next` | `super+g` | Goes to the next match |
| `find_previous` | `super+shift+g` | Goes to the match before |
| `find_match_case` | `alt+super+c` | Turns match case on or off, while the bar is up |
| `find_whole_word` | `alt+super+w` | Turns whole word on or off, while the bar is up |
| `find_regex` | `alt+super+r` | Turns regular expressions on or off, while the bar is up |

**Composer**, heard only in a composer:

| Action | Default | What it does |
|---|---|---|
| `bold` | `super+b` | Makes the selection bold |
| `italic` | `super+i` | Makes the selection italic |
| `code` | `super+e` | Makes the selection code |
| `link` | `super+k` | Makes the selection a link |
| `quote` | `super+shift+.` | Quotes the selected lines |
| `numbered_list` | `super+shift+7` | Makes the selected lines a numbered list |
| `bullet_list` | `super+shift+8` | Makes the selected lines a list |
| `task_list` | `super+shift+l` | Makes the selected lines a task list |
| `suggest` | `super+g` | Adds a suggestion, in a comment on lines of code |
| `toggle_preview` | `super+shift+p` | Switches between Write and Preview |

## Starting work

`+` at the top of the sidebar, or `Cmd-N` from anywhere, opens the new-work
view, titled **New worktree** in the bar: a box for what the work is, and
under it three chips.

- **Repository**: the `[[repo]]` entries in the file's order, a local one
  marked `(local)`. It opens on the open item's repository, else the last
  one picked. **Add repository…** picks a folder and appends a
  `[[repo]]` naming its repository to `config.toml`, without rewriting the
  file; Perch takes it up at once.
- **Base**: the branch to start from. For a repository with a root, the
  root's local branches and its remote's, with no network; for a GitHub
  repository without one, GitHub's. It opens on the repository's base
  branch.
- **New branch**: the name of the branch to cut, filled in from the message
  (`branch_prefix`, then its first five words) until you type in it. A name
  you typed that is taken is an error; one Perch filled in gets `-2`.

With a message, **Start** makes the worktree, opens it named for the
message's first line, and sends the message to the agent picked beside
it as the item's first Work run.
Without one, **Create worktree** makes the worktree alone. `Escape`, `Cmd-[`,
or the bar's X leaves the view and keeps what was typed until Perch quits.

Worktrees go under `~/.perch/worktrees/`, named for the repository and the
branch. A repository with a root is cut in the root, your own clone: the
branch is its branch, with your remotes and config, and `git worktree list`
there shows the worktree. Perch writes exactly this much in a root: the new
branch, git's record of the worktree, a fetch of the one base branch from its
remote, on Delete that branch, and when you open a pull request or press Push,
a push of the branch and the upstream it then tracks. The fetch uses your own credentials and
never Perch's token, and cannot prompt: a key that wants a passphrase fails it,
and the cut goes ahead from what the clone had, with a notice saying how old
that is. A GitHub repository with no root is cut in Perch's own clone, as
**Start work** on an issue or a pull request is. Start work on a worktree in a
local repository cuts in that repository.

### Start work from an item

**Start Work** is on the Worktree row of an item's details, behind the list
button in the bar: a button on an item with no worktree, and a row of the
worktree's menu on one that has a worktree. It is also in an item row's menu
and in the menu of a notification about a pull request or an issue. It cuts
a new branch from the item into a worktree of your own and opens it:

- From an **issue**, `perch/issue-<n>` from the default branch.
- From a **pull request**, anyone's and from any fork, `perch/pr-<n>-work`
  from the head Code shows: the pull request's worktree when it has one,
  commits not pushed yet and all, else its head on GitHub. The branch is
  diffed against the pull request's base, so it shows the whole change, and
  a pull request opened from it takes it over.
- From a **worktree**, a new branch from its head, named for it with `-2`.

An issue or a pull request leaves a prompt to work on it in the new
worktree's composer, and a second press opens the worktree the first made.
The new worktree is listed under **In Perch** on the item it came from. A pull
request opened from it says `Closes #<n>`, so merging it closes the issue or
the pull request it came from. A merged pull request, and anything
archived, has no Start work.

From a notification, Start work cuts from the item Perch has for its pull
request or issue, and makes that item first if there is none. It is greyed
once Perch has seen the pull request merged or the issue closed. A pull request
whose worktree was removed has to be opened first, which restores it. Other
kinds of notification have no Start work.

### Keeping work safe

A worktree made for work can hold the only copy of it.

- **Delete** asks again, naming the count, when the branch has commits no
  other branch, tag, remote-tracking branch or ref of Perch's holds. A branch
  pushed from Perch's own clone is not seen as pushed until a pull request
  brings its head back.
- **Archive** of a worktree asks first when the directory holds files git
  ignores, such as a `.env`, and names them: archiving removes the
  directory, and git keeps no copy of those. The branch and its commits stay.
  Worktrees with no pull request are never archived for age; pull request
  checkouts still are, after 14 days unopened.
- A pull request's worktree follows its head when the head moves only if
  the commit on disk is one the pull request had. A commit you made there
  and have not pushed keeps the worktree where it is, reported stale, as
  uncommitted changes do.
- A worktree of your own is never moved to its pull request's head. When the
  two are at different commits, the Branch row and the item's bylines say
  how: `3 unpushed`, `2 behind PR`, both, or `PR head not fetched` when the
  pull request's head isn't in the repository. Perch counts this in the
  worktree's own repository at each scan and open and fetches nothing to do
  it. **Submit review** and **Merge** are unavailable while the heads differ,
  since what is on screen isn't what GitHub would review or merge.
- **Update branch**, on the Branch row and in place of Merge, shows when the
  pull request's head isn't fetched or the worktree is only behind it. It
  fetches the pull request's branch, with your git credentials in one of your
  clones and Perch's token in its own, and fast-forwards the worktree to its
  head. A worktree with unpushed commits of its own isn't moved; the fetch
  only lets the row count both sides. Uncommitted changes to files the pull
  request changes stop the update.

## Opening a pull request

A worktree of your own with commits past its base has a **Pull request** line at
the foot of Conversation, where a pull request has its checks and Merge:
`Pull request · 3 commits ahead of main`. **Open pull request…** unfolds it
into a form:

- **Title**, filled in from the commit's subject when there is one commit, else
  from the item's name.
- **Description**, a composer's box going to GitHub, so `@` offers people and
  files as code spans. Filled in from the repository's pull request template
  (`.github/`, the root, then `docs/`), else the one commit's body, else the
  commits' subjects as a list. A branch Start work cut from an issue or a
  pull request ends it with `Closes #<n>`, as does a description the agent
  writes.
- **Base**, the branch it goes into: the one the branch was cut from, or any
  branch the repository has on GitHub.
- **Draft**, off.
- **Write with Claude**, or Codex, whichever the composer last used: the agent
  reads the branch under Ask's rules and writes a title and description into
  the form. Text you typed isn't written over; the form asks first. **Stop**
  ends it. The run isn't kept with the item's runs.

The form fills itself once, and keeps what you typed until Perch quits, whether
it's folded or another item is open. `Cmd-Enter` in the description, or **Open
pull request**, sends it:

1. GitHub is asked whether the branch has a pull request open already, as one
   opened on github.com that no poll has found yet. If it has, the row takes
   that one, and nothing is pushed or created.
2. The branch is pushed, never with force. A worktree of one of your clones
   pushes with your own git credentials and SSH key, never Perch's token, and
   the branch then tracks what it was pushed to. A worktree of Perch's own
   clone pushes with Perch's token.
3. The pull request is created, and the row becomes it in place, with its
   threads and runs. The open item turns into the pull request.

Uncommitted files are named and left out. A Work run in the worktree, a
worktree switched to another branch, or an empty title keep the button off. A
push the remote refuses, because its branch has commits yours doesn't, says to
pull or rebase in a terminal. A push your credentials can't make, such as with
an SSH key that wants a passphrase, says to push from a terminal; after that,
Open pull request skips the push. Neither agent can push or open a pull
request: Perch does it when you press the button.

On a pull request of yours whose worktree is ahead of it and not behind, the
Branch row has **Push** beside `3 unpushed`. It pushes the same way, and the
pull request is fetched again, so Submit review and Merge come back. The merge
box at the foot of Conversation has it too, as **Push 3 commits** where Merge
would be. Behind or diverged has no button, since that push would need force.

## Agents

Questions about an item or selected lines in its Code view run through Claude
Code or Codex in that item's worktree. The `+` in the gutter starts one on a
line, a removed line included, whose question is about the old file's lines;
so is a selection of nothing but removed lines, or one on the left of the
split layout. A line that already has a thread still offers the `+` under
the pointer, and the new thread's card stacks under the others. Follow-ups
remain attached to the local
conversation. A run is a question (**Ask**), which reads and answers, or, on
your own work, **Work**, which changes the worktree (see [Work](#work)).
Nothing an agent writes reaches GitHub unless you post it (see Posting).

### What the agent is told about the discussion

A question on a pull request or issue carries what was said on GitHub, as the
last fetch left it. The prompt quotes as much as fits in about 24,000
characters, in this order: each reviewer's latest verdict, the checks, the
threads within 20 lines of the lines asked about, the other unresolved threads,
the conversation tab, and the resolved threads. An entry goes in whole or not
at all; one comment is cut at 2,000 characters with a note of where the rest
is. Bots' comments stay out of the prompt. What does not fit is listed a line
each.

All of it, bots included, is also written as files to
`~/.perch/context/<run>/`: `INDEX.md`, `conversation.md` (the description,
comments, review summaries and timeline in time order), a file per review
thread under `threads/`, and `checks.md`. The prompt names the directory and
the agent reads what it needs. The directory is outside the worktree, belongs
to the run, and goes when the run is deleted; a retry reads the one its first
attempt had.

A follow-up that resumes the agent's session is told only what changed on
GitHub since the run it continues: new or edited replies in its thread, whole,
and new reviews, comments, threads and check results elsewhere. Nothing
changed, nothing is added. `perch --ask --print-prompt` prints the prompt a
question would get, and the files, without running an agent.

An item's runs are in the **Agents** row of its summary on Conversation, after
Checks, once it has had one. The row's pill says what they are doing, the
busiest first while any work goes (`1 running · 4 done`, with queued, waiting
on an approval, and failed), and after, how many there were, or that the last
one failed. A click unfolds the list under the rows: the working runs first,
then the item's history, newest first, twenty at a time with **Load older
runs**. A review's attempts are under a row for the review, which opens the
snapshot it read. Each run shows the agent, model, effort, queue or execution
time, and latest tool activity, and **Show in thread** goes to its thread. A
click opens the run in place to its retained request, ordered activity,
streamed response, usage and errors, scrolling inside a fixed height and
following the end while it runs; more than one can be open. Active runs can be
stopped, queued ones cancelled, and failed, stopped, or interrupted attempts
retried without replacing their history. Whether the row is unfolded, and
which runs are open, are kept with the item.

Every item's runs are behind the list button in the bar, before Review, under
the item's details, and in the bar of the pane with nothing open. It counts
the running and queued work, and has a dot while a run waits on you. Its
dropdown lists the working runs under their items' titles, then the fifteen
that finished last. A run there goes to its item on Conversation, with the
Agents row unfolded and the run open at the top. `Cmd-Shift-A` opens and
shuts the dropdown from anywhere, the arrows move through it, `Enter` goes,
and `Escape` shuts it. The notice a run leaves as it finishes has a **View**
that goes to it the same way, and **Show** in the Review popover, while a
review runs, goes to it in the Agents row. Work continues while another item is open.

Perch executes at most four top-level agents by default and queues the rest in
FIFO order; Gemini runs one at a time, and a waiting Gemini run does not hold a
slot from Claude Code or Codex. Stopping a run terminates and reaps its process group. Closing
Perch does the same for every process it owns; after an abnormal exit,
nonterminal rows return as Interrupted and are not replayed automatically.

Perch discovers all three CLIs automatically. Their optional settings are:

```toml
[agent]
binary = "claude"
model = "opus"
effort = "high"                # low | medium | high | xhigh | max | ultra
models = ["fable", "opus", "sonnet", "haiku"]
max_concurrent_runs = 4

[agent.codex]
binary = "codex"
models = ["gpt-6-astra"]       # omit to use Codex's own model catalog

[agent.gemini]
binary = "agy"
models = ["gemini-4-carbon"]   # omit to use agy's own model catalog
```

Without Claude Code installed, a Claude `model` gives way to `gpt-6.1-sol`
when Codex offers it, at the same effort, or to `agy`'s first Gemini model
when only `agy` is installed. For heavy PR review usage with Gemini, prefer
setting `GEMINI_API_KEY` (or `GOOGLE_API_KEY`) over a personal subscription
OAuth login in `agy`.

`perch --agents [limit]` prints active runs and retained history without
launching an agent. `perch --ask owner/repo#N path:start-end "question"`
runs the same question flow from a terminal; omit the path to ask about the
whole item, and add `--print-prompt` to see the prompt without running it.

### What the agent can and cannot do

An agent reads text nobody vetted: the pull request, its comments, and the
repository itself. Any of it can contain instructions aimed at the agent.
Perch does not try to detect them; it limits what following one could do.

- **Read-only tools.** Claude Code gets `Read`, `Glob`, `Grep`, `LS`, and
  `Bash` for `git diff`, `git log`, `git show`, and `git blame` only. `Edit`,
  `Write`, `MultiEdit`, `NotebookEdit`, `WebFetch`, and `WebSearch` are
  denied by name, so no setting can allow them. Anything else is refused
  without a prompt, and the refusal shows up as activity. Codex runs in its
  `read-only` sandbox.
- **A sandboxed shell.** Every Claude Code command runs inside Claude Code's
  own sandbox with no network access and no write access anywhere, the
  worktree included: `git diff --output=` fails like any other write. There
  is no unsandboxed retry, and a machine that cannot start the sandbox
  refuses to run rather than running without it.
- **No repository-supplied configuration.** Claude Code reads only
  `~/.claude/settings.json`, never the repository's `.claude/settings*.json`
  or `.mcp.json`, and starts no MCP server. Codex is told the worktree is
  untrusted, so its `.codex/` config, hooks, and rules are not loaded even
  when you have trusted the repository elsewhere, and it runs without
  `~/.codex/config.toml`, plugins, or hooks, since the MCP servers named
  there run outside its sandbox. Reviews run Claude Code in `--safe-mode`
  as well.
- **No secrets.** The agent does not inherit `GITHUB_TOKEN`, `GH_TOKEN`,
  Perch's own git token, or any variable named `*_TOKEN`, `*_SECRET`,
  `*_KEY`, `*_APIKEY`, `*_PASSWORD`, `*_PASSWD`, `*_PASS`, `*_PAT`, or
  `*_CREDENTIALS`. The CLIs' own credentials (`ANTHROPIC_API_KEY`,
  `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `OPENAI_API_KEY`,
  `CODEX_API_KEY`, `GEMINI_API_KEY`, `GOOGLE_API_KEY`) are the exception, since without them there is no run.
  Credentials for other providers, such as `AWS_SECRET_ACCESS_KEY`, fall
  under the rule, so a CLI configured that way must be signed in instead.
- **Cleaned quotes.** Text quoted into a prompt keeps the words a reader
  sees and loses the rest: HTML tags and the comments GitHub renders as
  nothing, link titles and image alt text, and invisible formatting
  characters, however they were written. It is set between
  `[BEGIN untrusted text: …]` and `[END untrusted text]` lines, a line
  inside it that imitates a marker is spoiled, and your question comes
  last. The discussion files under `context/`, which the prompt says are
  untrusted like a marked block, and the review history export under
  `knowledge/reviews/` are cleaned the same way.
- **Inert links.** A link in an answer or a comment does nothing until it
  is clicked. Hovering it shows the full destination; a click opens `http`
  and `https` links in the browser and a file of the worktree in Code
  (see [Links to code](#links-to-code)), and nothing else.

What remains: the CLIs still read the repository's `CLAUDE.md` and
`AGENTS.md` as instructions, along with your own user-level configuration,
and the agent can read any file your user can, not only the worktree. What
it reads can reach you only through its answer.

### Work

A composer on your own work, a worktree with no pull request or a pull
request you opened, has an access picker after the effort: **Read only**,
which is Ask, then the Work levels under [Approvals](#approvals). Anything
else, someone else's pull request, an issue, a review's snapshot, is Ask-only,
and the backend refuses Work there whatever the picker says.

On an issue the picker is there for a new question, with **Read only** and
**New branch**, but Work does not run there: the default branch the issue
reads is shared by every issue of the repository, and has no branch to commit
to. New branch's send reads **Start work**,
and cuts `perch/issue-<n>` as the item's Start Work does, with what you wrote
in the new worktree's composer in place of the usual last line of the prompt
(after the issue's number, title, link and body, and the lines you asked
about, if any), to send or edit. The issue's next question starts on Ask
again. A new question
starts on what you last picked on the item, else Work on a worktree Perch made
and Ask elsewhere; a follow-up starts on its thread's last run. A session is
resumed only by a run of its own kind: a move between Read only and Work
starts over with the thread folded in, as a switch of agent does.

A Work run edits files and runs commands in the worktree, and is told it may.
It keeps everything under [What the agent can and cannot do](#what-the-agent-can-and-cannot-do)
except the first two points, and it can ask for more (see
[Approvals](#approvals)).

- **Claude Code** runs with its prompt and Perch's answers on stdin
  (`--input-format stream-json`, `--permission-prompt-tool stdio`). It may
  use `Edit`, `Write`, `MultiEdit` and `NotebookEdit` under the worktree,
  and every command runs in its sandbox without asking. `Bash` itself is
  not on its allowed list, since there it would let a command leave the
  sandbox without asking. The sandbox writes the worktree, the temporary
  directory and `[repo.work] write`, never the worktree's `.git`, `.claude`,
  `.codex` or `.mcp.json`, nor the git directories a linked worktree uses,
  so `git commit` fails. It reaches `[repo.work] network` without asking.
- **Codex** runs as `codex app-server`, the protocol its own app speaks,
  with plugins, hooks and apps off and each MCP server in your Codex config
  turned off by name, since the app-server has no way to ignore that
  config. Its thread runs under a permission profile: everything readable,
  the worktree, the temporary directory and `[repo.work] write` writable,
  the same paths read-only, and no network until it asks. Codex lets a bare `git commit`
  write the repository's objects and refs whatever the profile says, so a
  Codex Work run can commit; it cannot change git's config or hooks, and it
  cannot push.

Neither is given a token or a route to GitHub. A machine without the sandbox
refuses the run. One Work run at a time runs in a worktree; another waits,
saying so, without taking a slot, and questions go on beside it.

On Linux, Claude Code's sandbox is bubblewrap with socat for its network
proxy. A Work run or a question on a machine without `bwrap` and `socat` on
`PATH` is refused before it starts: "Claude Code's sandbox needs bwrap and
socat on Linux…". Claude Code on Linux also skips the glob that keeps a Work
run from making a `.git` inside the worktree, and on any platform a run can
move in a directory that already holds one, so Perch looks itself, as a Work
run starts and again as it ends: a `.git` that appeared below the worktree's
top during the run, or one there before whose `config`, `config.worktree`,
`commondir`, `gitdir` or hooks changed, or whose `.git` file now points
elsewhere or at a repository that changed, fails the run, with its path
named. `.GIT` counts, as macOS doesn't tell it from `.git`. A submodule that
`.gitmodules` named before the run, checked out by it with a `.git` file
pointing into the repository's `modules/`, is left alone, and
`node_modules`, `target`, `.venv` and `venv` are not looked through. The
look follows no link out of the worktree and opens nothing but plain files,
a link or a FIFO counting by what it is, `.gitmodules` included, and goes
through at most 200,000 folders and hooks in nested repositories: a run that
takes the worktree past that fails too.

### Approvals

When a Work run needs something its sandbox refuses, it asks. By default
(`[work] approvals = "auto"`) the CLI's own automatic reviewer answers first:
Claude Code runs in its auto permission mode, and Codex with its `auto_review`
reviewer. What the reviewer decides shows in the run's steps: Codex's as
`Approved automatically: …` or `Refused automatically: …` with its reason,
Claude Code's as the command itself, `Ran outside the sandbox: …` or `Ran …
(network: example.com)`. Claude Code's auto mode needs a model that has it: Fable, Opus and Sonnet do
(checked with Claude Code 2.1.280); Haiku does not, and with it every request
comes to you. Anything the reviewer passes on comes to you, and so does everything when
`approvals = "ask"`. A repository's `[repo.work] approvals` stands over
`[work]`'s, and only there can it be `"never"`: Perch allows every request
without asking, and each shows in the run's steps as `Allowed without asking:
…`. The run still starts in its sandbox, and the protected paths (`.git`,
`.claude`, `.codex`, `.mcp.json`) stay denied.

The access picker beside the effort shows who decides for the item, after
Read only: **Ask me** (`"ask"`), **Auto-review** (`"auto"`) or **Allow all**
(`"never"`, in red). It starts on config's level. A pick holds for that item
until Perch quits and changes nothing in config.toml; picking Read only keeps
it for the next Work. Allow all, on or
off, applies to the next request, even one from a run already going; the
switch between Ask me and Auto-review applies from the next run, since the
CLI is started with it. An agent's byline says the level its run has, after
the time: the same four.

What comes to you: Claude Code asks
for a host a command wants to reach, for a command it wants to run outside the
sandbox, and for an edit outside the worktree. Codex asks to run a command
outside its profile, naming the host when it knows the command needs one, and
for file changes and permissions it lacks. The request is a card in the run's
block, `Connect to example.com` or `Run outside the sandbox` with the command
in the code font and the agent's reason, with three buttons:

- **Allow once** allows this request.
- The **Always** button allows it now, and keeps a grant on the repository
  that answers the same request again without asking: **Always allow
  example.com here**, **Always allow this command here** for that command
  exactly, **Always allow this here** for a tool or a path, and for a git
  command that writes the repository, **Always let Work commit here**.
- **Deny** says no. The agent is told "The reader said no." and goes on.

A request not answered in time is denied: fifteen minutes in the app, eight
hours under `perchd`, where nobody may be connected, or
`[work] approval_timeout_mins`. The agent is told how long it waited. A
request waits in the engine, not in a window, so a window that opens or a Mac
that connects while it waits shows the card, and the first answer from any
of them settles it for all. A run that is stopped or ends settles whatever it
was waiting on. The run's steps keep what was asked
and what was answered. While it waits, its item's Agents row counts it as
waiting and the bar's Agents button has a dot.

Grants are Perch's own, kept in its database and keyed by repository. Perch
never writes Claude Code's settings or Codex's rules to remember one. The
summary of an item that takes Work runs lists its repository's grants in a
**Work may** row, each with a × that takes it back from the next request on.

Without the commit grant, a Work run is told to commit only when you ask it
to, and a Claude Code run then asks to run its `git commit` outside the
sandbox; **Always let Work commit here** on that request is the grant. A
request counts as a commit only when it is plain git: no option before the
subcommand, such as `-c` or `-C`, and none after it that signs, runs a program
or sets config, such as `-S`, `--exec` or `--template`. Anything else is asked
for as its own command, and the grant does not answer it.
The commit grant also changes the sandbox of later Claude Code Work runs: their
worktree's git directories become writable, apart from each one's `config`,
`config.worktree`, `commondir`, `hooks` and `modules/` (its submodules'
repositories), and the repository's other worktrees' directories under
`worktrees/` (and the `commondir` and `config.worktree` of any added while the
run goes). On Linux, where that last rule can't be kept, a linked worktree's
run gets only its own directory and the repository's `objects/`, `refs/` and
`logs/`; git there prints that it can't lock `packed-refs` and commits anyway,
and the run is told to expect that line. A command that rewrites `packed-refs`,
such as deleting a packed branch, fails there, and the run can ask to run it
outside the sandbox.
The run is told it may commit but not push. A follow-up that resumes a
session begun before the grant says so ahead of the question, since the
session keeps what it was told when it began.
The grant doesn't answer a Claude Code or Codex run's request to run git
outside the sandbox: the run keeps working inside the sandbox while git runs,
so it could move a repository into the worktree, and git outside the sandbox
would run what that repository's config says. You're asked, and the card says
why. Gemini, which has no sandbox and asks before every command, commits by
the grant, except while a commit would run hooks, or once a `.git` has
appeared, changed or gone out of sight below the worktree's top since the run
started, or the worktree is too big to look through.
With or without the grant, a Work run, Claude Code's or Codex's, can't write a
`.git` below the top of its worktree: git would take a directory holding one
for a submodule and, the next time it runs there outside the sandbox, use a
config the run wrote. Codex can't read one there either, since its profile
only denies a pattern outright. A run that needs a repository of its own makes
it in the temporary directory. A worktree that `git worktree add` makes from
the same repository below the top, as another Claude Code session does in
`.claude/worktrees/`, is the repository's own and doesn't count. Neither
sandbox stops a run moving in a directory that already holds a `.git`, so Perch's own `git status` and
`git diff` never look inside a submodule for edits: one shows as changed only
when its commit does, and one with a `.git` in it keeps its worktree from
being reset or removed.
`[repo.work] trust = true` loads the repository's own agent configuration for
Work runs: Claude Code reads its project and local settings and its
`.mcp.json`, and Codex treats the worktree as trusted, so its `.codex/` loads.
What those files name runs as you, outside any sandbox, which is why trust is
off unless you turn it on. Questions and reviews never read it.

As a Work run starts and as it ends, however it ends, Perch records the
worktree as a commit under `refs/perch/runs/<run>/` in the repository, without
touching the worktree, its index or `HEAD`. The run's card says `Work` and how
many files it changed; a click shows the change on Code as the **What the run
changed** scope. The scope menu's **Last run** is the newest Work run that
changed something, passing over runs that changed nothing, and moves on to the
next such run as it finishes. Deleting the run's thread or item drops the refs.

## Worktree reviews

**Review** in the bar opens one popover for the whole review. On top are the
agents: on a pull request, one line naming the reviewers the next run starts,
with **Run**, and **Settings** to choose them, the range and the
instructions. Each reviewer is one line: a box to take it or leave it, its
name, then its model and effort side by side; one that cannot run says why
under its name. Under them is your own review for GitHub: the
drafts, a summary, and a verdict to send them with. Unfolding the settings folds
your review to a line, and **Open** brings it back. A worktree without a pull
request has only the agents, their settings showing. The button is dark while
there is a pull request to review, counts your drafts, and says how far a
running review has got.

Running the agents starts Claude Code and/or Codex's native review workflow
against the branch diff from the selected base's merge base to the head. It
works for Perch-managed and registered external worktrees, with or without a
pull request. **Changes** in the settings reads head → base, as a review's
history does. The head says where the range ends: **Last
commit** is `HEAD`, and **Working tree** adds the uncommitted work, staged,
unstaged and untracked files alike, as a commit Perch makes over `HEAD`
without touching the worktree, its index or its refs. Head starts on Working
tree when the worktree is dirty and the branch has no commits yet, or the
Files view shows an uncommitted scope; otherwise on Last commit, and under a
dirty worktree's range Perch notes `Uncommitted work is left out`.
Staged-only, individual-commit, and incremental-only review scopes are not
supported. If the diff is empty, Perch records that result without launching
an agent.

Findings from a review of the working tree show in the Files view under
Uncommitted and Branch while the checkout is still on the commit the work sat
on, each where its lines read the same as what was reviewed: on the new side,
in a file unchanged since; on the old side, when the view diffs from the
review's merge base. The rest stay in Conversation.

The instructions box in the settings tells the reviewers what to focus on, such as
"review rfcs/multi_get.md only" on a branch that also carries a prototype. The
range reviewed stays the same: Claude reads the instructions after the range in
its `/code-review` argument, and Codex, which refuses a custom prompt beside
`--base`, reads them as developer instructions. A review keeps its
instructions, so **Retry** uses them again and its history card shows them.

A review of a pull request also reads its discussion. When the snapshot is
taken, Perch writes the discussion to `~/.perch/context/review-<id>/` in the
same shape a question gets it, and tells each reviewer, ahead of the requester's
instructions, where it is and what to do with it: not to report a problem a
reviewer already raised in an unresolved thread, and to report a change a
reviewer asked for that the range does not make, naming who asked. A review of
a worktree with no pull request has no discussion to read.

Every review uses a clean, read-only, immutable snapshot in Perch-owned
storage. Perch retains its exact diff, manifest, commits, and anchored blobs so
history remains readable after the branch moves or an external checkout is
deleted. Opening a historical review never changes the current worktree.

Perch currently supports Codex CLI 0.155.1 or newer through `codex exec review`
and Claude Code 2.1.223 or newer through `/code-review`, when the installed CLI
advertises the required local, noninteractive options. Missing, older, or
incompatible CLIs are shown as unavailable with a reason; Perch does not fall
back to a generic review prompt. Review mode excludes Claude's hosted `ultra`
mode and never enables fix, comment, or permission-bypass options.

Findings are durable local discussions, separate from the attempts that found
them. Repeat reviews preserve every group, attempt, observation, failure, and
valid zero-finding result while reconciling the same problem back to its
existing thread. **Retry** creates another attempt with the original snapshot
and settings. **Review latest** creates a new group and immutable snapshot from
the branch's current state.

On an open pull request, a new finding on lines of its diff arrives as your
draft on those lines, in place of the reviewer's message. It is a draft like
one you write (see Posting): you edit or delete it, and it posts only when you
submit the review. The card above it still gives the severity and the
reviewer. Deleting the draft dismisses the finding, and dismissing the finding
or linking it as a duplicate deletes the draft. GitHub takes a review comment
only on a line its diff shows, a changed line or one of the three around it,
so this needs the review to be of what GitHub has: the pull request's head,
diffed from the same merge base. Any other finding stays in Perch as the
reviewer's message, as does every finding on a worktree without an open pull
request.

Follow-ups stay local to Perch unless you post one yourself. Reviews do not
resolve threads on GitHub, modify code, run automatically after a push, or use
a hosted review service. Perch also exports each item's review history as
deterministic Markdown at
`$PERCH_HOME/knowledge/reviews/<owner>/<repo>/<item-id>.md` (under
`~/.perch` when `PERCH_HOME` is unset). Re-exporting replaces that stable
document rather than duplicating unchanged entries; unrelated legacy files in
the knowledge directory are not imported.

## The file tree

The drawer on the right holds the open item's files as a tree, on Code. It is
a column beside the editor, so opening it moves the file over rather than
covering it, and the open file, its scroll and a composer are kept through it
opening and shutting. When the window is too narrow for both the drawer and a
usable pane, the drawer lies over the pane's right edge instead, from under
the tabs to the status bar.

The tile at the right end of Code's tabs, after the layout tiles, opens and
shuts it, as Codex has its folder there, and so do `Cmd-Shift-E` from anywhere
and the drawer's own close button. From Conversation, `Cmd-Shift-E` goes to
Code with the tree open. On Conversation the tree is hidden, and it comes back
with Code. It waits for an opening item to be ready. It opens by itself on an
item's first visit to Code, and after that stays as it was left. Whether it is
open and how wide it is are remembered between launches.

The tree is the changed files until its filter is turned off, with the scope
button and the viewed count above it. An item opens on the scope last picked
from that button, as Codex does, when the item has something under it, and on
the branch otherwise; a single commit, or a run's changes opened from its card,
is not remembered. A file opened from the tree opens on
Code, and a file opened from anywhere else, a card or a definition, is shown
in the tree: the way to it opens, the filter comes off when the file is not a
change, and the tree scrolls to its row. During a retained snapshot the tree
has no scope to pick and nothing to mark viewed, and the way back to the
current files is in the tab strip.

## Going to a file

`Cmd-P` puts VS Code's Go to File over the top of the pane, from Code or
Conversation, for any item with files. Before anything is typed it lists
the files the item has had open, the most recent first, and then its
changes. As you type, it lists the recently opened files that match and
then every other file in the worktree that does, with the matched letters
in blue and a change's status letter at the right.

The ranking is VS Code's own. The letters must come in order. A run of
them beats the same letters spread out, a match in the name beats one
that needs the directory, and a name that starts with the query comes
first, shorter names before longer. A `/` in the query matches against
the whole path, words separated by spaces must all match, and a query in
double quotes must match as one run. A recently opened file is listed as
recent only when the letters form one run in its name, or in its path for
a query with a `/`; otherwise it is ranked with the other files.

`Enter` opens the file on Code, and `config.rs:42` opens it at line 42;
`:42` alone goes to line 42 of the open file. The list of recently opened
files is kept for each item until Perch quits.

| Action | Key |
|---|---|
| Go to File | `Cmd-P` |
| Next, previous | `Down`, `Up`, or `Ctrl-N`, `Ctrl-P`; `Cmd-P` again for next |
| Open | `Enter` |
| Close | `Escape`, or a click outside it |

`Cmd-P` is `go_to_file`, which can be put on another key: see
[Keyboard shortcuts](#keyboard-shortcuts). The key it is on walks down the
list. The others are the palette's own.

## Code navigation

Code reads like an IDE when rust-analyzer is installed. Rest the pointer on
an identifier and a tooltip says what it is: the signature in the code font,
then the first lines of its doc comment. The tooltip cannot be entered and has
no scroll. It opens where the pointer is and goes away when the pointer does,
so nothing has to be dismissed; the panel under the editor is where a list
goes, and the tooltip is a glance. A reader who wants the whole doc comment
goes to the definition.

A definition in a dependency or the standard library opens as a tab like any
other, with a chip naming where it is from (`tokio 1.40.0`, `std`, `git:
floem`). Such a tab is a place to read: whole, with no diff, no Viewed and no
threads, and it is not remembered when the item is closed.

The keys are VS Code's. Right-click an identifier for the same entries, each
naming the symbol it acts on; on whitespace the menu is Copy alone.

| Action | Key |
|---|---|
| Go to Definition | `Cmd-click`, `F12` |
| Go to Type Definition | menu |
| Go to Implementations | `Cmd-F12` |
| Find All References | `Shift-F12` |
| Show Call Hierarchy | `Shift-Alt-H` |
| Back, Forward | `Ctrl--`, `Ctrl-Shift--` |
| Close the panel | `Escape` |

Each but `Cmd-click` and `Escape` can be put on another key, and Go to Type
Definition given one: see [Keyboard shortcuts](#keyboard-shortcuts).

One location opens; several open the panel and the reader picks. The call
hierarchy is a tree: a chevron on a row asks for that row's callers, and a
toggle in the panel's header turns it to callees. The caret resting on an
identifier tints its other uses in the file. All of it is read-only, and all
of it is rust-analyzer: while the server is absent, failed or waiting for you
to trust the code, the status line says so and nothing opens. A goto over an
absent server offers the install again, and one over a server waiting for
trust offers **Trust and start** again.

`Cmd-F` puts a find bar over the top right of the code, as in VS Code: a box
for the text, toggles for match case, whole word and regular expression, the
count, and arrows. Every match in the open file is tinted, the current one
darker and selected, and the editor scrolls to it; as the text is typed the
bar moves to the nearest match from the caret, and a match inside a folded
run reveals the run. `Cmd-F` with something selected on one line looks for
that. The bar stays up from one file to the next, looking for the same text
in each, and searches a diff's removed lines along with the rest in the
stacked layout.

| Action | Key |
|---|---|
| Find | `Cmd-F` |
| Next match, previous match | `Enter`, `Shift-Enter`, or `Cmd-G`, `Cmd-Shift-G` |
| Match case, whole word, regular expression | `Alt-Cmd-C`, `Alt-Cmd-W`, `Alt-Cmd-R` |
| Close the bar | `Escape` |

The bar's own `Enter`, `Shift-Enter` and `Escape` stay; the other keys can
be changed: see [Keyboard shortcuts](#keyboard-shortcuts).

`Page Up` and `Page Down` scroll the open file or Conversation by a page, keeping a line of the last in sight, and leave the
caret where it is. They work from the code, the page or a composer. `Cmd-Up` and `Cmd-Down` go to the
top and the end, the caret with them in the code; a composer keeps them for
its own text. The find bar and the other boxes keep all four.

An SVG opens as the picture it draws rather than as its markup, fitted to the
pane. A changed one shows before and after side by side, both at one scale,
so a change of size shows as one; an added or deleted one says which it is.
The status line gives the picture's size. The Source tile at the right end
of the tab strip shows that file's markup instead, with its diff, gutter and
threads, and the picture is back at the next click.

## Links to code

A file, a line or a symbol of the worktree that a comment or an answer
names is a link to it, on Conversation and in the cards over the code: in
a code span, in a fenced block, or as a link's destination. A click goes to
Code, opens the file with the caret on the line, and `Ctrl--` comes back.
Hovering says where it goes. It is drawn as any link is, so the text reads
as it did.

A file links when the worktree has it. A line follows the path as the
compiler writes one, `src/db.rs:17` or `src/db.rs:17:5`, or as GitHub
does, `src/db.rs#L17`; a range opens at its start, and the line is the
head's, which is what an agent reads. A path can be written short,
`wal/mod.rs:303` or `gc.rs`, and links when only one file ends with it, or
only one of the changed files does. An absolute path links only inside the
worktree: nothing outside it opens from a comment.

A symbol links through rust-analyzer's workspace symbol search: a name,
`WalGcPolicy` or `list_wal_ssts`, or a name after what it belongs to,
`Db::open`, `WalGcMode::Regular`. One definition opens and several open the
panel as its definitions, generated code left out when there is code
written by hand. Fields are not in the search, so `DbOptions.ttl` links
`DbOptions`. A lone word in lower case, such as `inner`, is looked up only
when it is called, `inner()`, since it is as often a field or a variable;
and a name with more than ten definitions, such as `new`, links nothing.
In a fenced block the paths link whatever the language, and the names that
read as types do when the block is Rust, a diff, output or of no language
named. A name links
once rust-analyzer has answered, a moment after the text is drawn; while
the server is absent, failed or waiting for trust, only files do.

## One engine to a Perch home

Everything below the window, the GitHub client, the cache, the checkouts,
the language servers and the agents, is Perch's engine. Whatever runs it,
the app or `perchd`, holds an exclusive lock on `$PERCH_HOME/engine.lock`
while it runs and writes its process id there. A second engine on the same
home refuses to start and names the process that has it: the app opens its
window and says "Perch could not start its engine: another Perch engine
(process …) is running on …", and nothing runs behind it. The lock goes
with the process however it ends. On a file system that has no locks, some
network ones, the engine runs anyway, warns in the log, and nothing stops a
second one. The commands that only read the cache,
`perch --items`, `--agents`, `--timeline`, `--scope` and `--scan`, need no
lock and run beside an engine.

## perchd

`perchd` runs the engine with no window, on a machine the Mac reaches over
ssh. It is built from the same crates as the app, without a UI toolkit, so
it needs no GTK. It is built for Linux and for a Mac, on x86_64 and arm64,
as `perchd-linux-<arch>.tar.gz` and `perchd-macos-<arch>.tar.gz`. Linux's
is built against musl and linked statically, so it runs on any Linux of
its architecture, Alpine included, whatever its libc. Every app has all
four built into it and brings the one a host needs to it (see [The app's
own engine](#the-apps-own-engine)), so nothing is installed on a host first.
Releases publish them too, with the SHA-256 of every asset in the release's
`SHA256SUMS`: to run one by hand, put it in `~/.local/bin`.

- `perchd serve` runs the engine and listens on
  `$PERCH_HOME/run/perchd.sock` (the directory owner-only, the socket mode
  0600). It keeps polling, sweeping and running agents with no client
  connected. With `--exit-after-idle <seconds>` it stops by itself once it
  has had no client connected and no agent run queued or under way for
  that long: how a bridge starts one (below). Run by hand with no flag,
  it stays until it is stopped. SIGTERM, SIGINT or SIGHUP stops it as quitting the app does:
  running agents are stopped and recorded Interrupted, the socket is
  removed and the home let go. A bridge of another build asking it to stop
  over the socket stops it the same way. A second SIGTERM or SIGINT while
  it stops ends it at once; another SIGHUP does not, as a dropped ssh
  session sends two. It logs to stderr, in colour only on a terminal, and
  to `$PERCH_HOME/logs`, never to stdout. With another engine on the home
  it exits 75. It does not start on a `perch.db` a newer Perch has
  migrated, which it would otherwise run without, its runs unrecorded.
  When it can't start, it writes why to `$PERCH_HOME/run/serve-failed`
  (mode 0600) as well as to its log, and removes the file when it next
  starts.
- The engine never keeps the ssh session's `SSH_AUTH_SOCK`,
  `SSH_CONNECTION`, `SSH_CLIENT` or `SSH_TTY`, whether a bridge started it
  or it was run by hand in a login: it outlives the session, and every
  agent it starts would get them. git over ssh on the host uses the
  host's own keys. An engine run by hand outside an ssh session, from a
  desktop login on the host say, keeps `SSH_AUTH_SOCK`, which is then the
  host's own agent.
- `perchd bridge` joins its stdin and stdout to the socket, byte for byte,
  until either side closes. It is what the app has ssh run. With
  no engine answering it starts `perchd serve --exit-after-idle 900` in a
  session of its own, so the engine outlives the ssh connection, and waits
  up to 10 seconds for it. That engine stops 15 minutes after its last
  client has gone and its last run has ended (see [What runs
  where](#what-runs-where)); `PERCH_IDLE_EXIT_SECS` in the host's login
  environment sets another wait. An engine that failed to start for a reason it left in
  `serve-failed` is not waited for: the bridge says that reason. When
  something holds the home's lock without listening, an engine still
  stopping or the Perch app running on the host, it names that process.
  It writes nothing but the engine's messages to stdout. A bridge the app
  put there, one run from `$PERCH_HOME/bin/perchd-<build>`, marks its own
  file as used now, and removes the other `perchd-*` there that nothing
  has used in a week, and the temporary files of uploads that stopped
  over an hour ago. When a build was last used is its file's modification
  time.
- `perchd bridge --replace` makes way for its own build. An engine of
  another build is asked over the socket to stop, and stops as SIGTERM
  stops it. The bridge waits up to a minute for its socket to stop
  answering and the home's lock to be free, then starts its own engine
  and carries on as a bridge; if another bridge has started an engine
  there meanwhile, it joins that one. An engine of its own build is
  joined as it is. Builds are the same or they are not: which version is
  the later is never asked. `--replace-if-idle` does the same only when the engine has no runs
  and no clients; otherwise it writes `perch-busy: <version> <runs>
  <clients>` to stderr and exits 98, leaving the engine running. Nothing
  is ever signalled or killed by its pid. The Perch app running on the
  host holds the home with no socket, and can't be replaced: quit it
  there. A build is the `<build>` of the `perchd-<build>` it runs as, the
  first 12 hex digits of the binary's SHA-256, or its version when no app
  put it there.
- `perchd status` connects without starting anything and prints whether
  an engine is running, its version, protocol and build, its Perch home, how many
  other clients are connected and how many agent runs are queued or under
  way. It exits 0 when one answers and 3 when none does.
- `perchd --version` prints the version and the wire protocol, as
  `perchd 0.9.0 (protocol 6)`.

A socket file left by an engine that was killed is removed when the next
one starts, once nothing answers on it.

A bridge is the only thing that starts an engine on a host. After the
host boots, after an engine crashes, or once one has stopped for having
nothing to do, nothing runs there until the app next connects. Stopping the engine while the app is connected does not
keep it stopped: the app reconnects and its bridge starts another. Quit
the app, or set it to This Mac, first. On Linux the engine needs logind's
`KillUserProcesses=no`, the default on most distributions; with `yes`, it
is killed when the ssh session that started it ends, its runs with it,
unless the user is in `KillExcludeUsers`. A user `KillOnlyUsers` names is
killed whatever `KillUserProcesses` says. The engine checks this (see
[What the app checks](#what-the-app-checks)).

The app says it is still there every 15 seconds it has nothing else to
send, and the engine hangs up on a client it has heard nothing from in a
minute: a Mac asleep, or off the network, would otherwise hold its windows'
open items and language servers until TCP gave up on it, hours later. A
client that connects and never says hello is hung up on after the same
minute. The Mac's windows reconnect when it wakes. While the app is
connected to a host it opts out of App Nap, which could hold its keepalives
back while it is hidden.

## An engine on another machine

### Setting up a host

The host is Linux or a Mac, on x86_64 or arm64, and reachable with `ssh
<host>`. The app brings its own `perchd` and starts it there (see [The
app's own engine](#the-apps-own-engine)), so nothing of Perch's has to be
put on it first. On it:

1. On Linux, install bubblewrap and socat (`apt install bubblewrap socat`,
   or your distribution's), which Claude Code's sandbox needs there.
2. Sign in to what the engine uses, as the user it runs as: `gh auth login`
   (or `token` in its config.toml), and each agent CLI you run, `claude`,
   `codex` or `agy`. Nothing is copied from the Mac.

Open **Settings > Environment** in the window you want to change. Enter
the SSH host and select **Connect**. That window opens the host’s
repositories and agent runs. **Use local environment** brings it back to
this machine. Switching closes the current item. Other windows keep their
environment and open items.

Your login at the foot of the sidebar is the one signed in to GitHub in
the window’s environment, with the environment’s name under it. Click it
for a menu of environments: this machine, each host a window has
connected to and any host a window has open, with the window’s own
checked. Choosing one switches the
window to it. **Connect to SSH host…** opens **Settings > Environment**
for a new host. The × on a host no window has open takes it out of the
menu. The hosts connected to are kept in `hosts.json` in this machine’s
Perch home.

Perch reopens the windows from your previous session, including each
window’s environment, size and open item. A remote window reopens its item
when its host connects. Closing one window removes it from the session.
Closing the last window keeps it for the next launch.

**New Window** inherits the environment of the window in front. On a first
launch with no saved windows, Perch uses the existing `[remote] host` in
the local config if one is set, or opens locally.

The host is named as ssh is given it: a name from `~/.ssh/config`, or
`user@host`. A port goes after it, `chris@devbox:2222`, or the whole can be
a URL, `ssh://chris@devbox:2222`; ssh itself takes a port only as an
option, so Perch passes it as one, to the connection, the install and Open
in Terminal alike, and Zed's link carries it. VS Code's does not: with a
port, or an IPv6 address, Open in Editor needs Zed, or a name for the host
in `~/.ssh/config`. An IPv6 address with a port is in brackets,
`[fe80::1]:2222`. ssh must connect with no question asked, a
password or a host key to accept: Perch runs it in batch mode.

A Mac that is a host is kept from idle sleep for as long as its engine
runs (`perchd` holds `caffeinate -i` on itself), since its work is done
with nobody at it. Shutting a laptop's lid still sleeps it.

Repositories are added from the app (see [Adding a repository on a
host](#adding-a-repository-on-a-host)); nothing has to be cloned or
written on the host first.

### What the app checks

The engine on a host looks at the host as it starts, again whenever its
config.toml changes or it takes up a new GitHub token, and tells every
connected app what it found. **Settings > Environment > Host setup** shows
required checks that need attention, with instructions you can copy.
**Show details** lists passed checks and optional tools. A setup issue
also appears at the foot of each window using that host, with an **Open
Settings** button. `perch --connect` prints
the same list. It checks:

- **GitHub token** (required): one is found, as the engine finds it:
  `GITHUB_TOKEN`, `GH_TOKEN`, `gh auth token`, then `token` in its
  config.toml. Only whether there is one, and where it came from, is
  said; the token never leaves the host. Fix: `gh auth login`.
- **git** (required): `git --version` runs. Fix: the install command for
  the host's package manager (apt-get, dnf, yum, pacman, zypper or apk).
- **Repositories** (required): at least one `[[repo]]` has a `root` that is
  a directory on the host. Roots that are not there are named. For a
  github.com repository the fix is the `git clone` that puts it there.
  While it fails, its row has an **Add repository…** button, which goes to
  Settings > Repositories.
- **Claude Code**, **Codex**, **agy (Gemini)**: each agent CLI as the
  engine's agent discovery found it, with its path and version. None is
  required while another is installed; with none at all, Claude Code is.
  Fixes: `curl -fsSL https://claude.ai/install.sh | bash` and `npm install
  -g @openai/codex`; agy points at its documentation.
- **bubblewrap and socat** (required, on Linux with Claude Code): both on
  `PATH`, for Claude Code's sandbox. Fix: the package manager's install.
- **logind** (required, on Linux): `loginctl show-session` says logind
  does not kill the user's processes at logout. Fix: a drop-in that adds
  the user to `KillExcludeUsers`, then a restart of `systemd-logind`. With
  no `loginctl`, or no logind to ask, as in a container, there is no
  check.

Each command runs with a timeout of ten seconds, beside the engine's work,
and one that runs out counts as missing. The app's own engine, on the Mac,
checks nothing.

### The app's own engine

With no `[remote] command`, the app runs its own `perchd` on the host. It
carries one for each system and processor inside its own binary, put there
when the app was built (see [How an app gets its
engines](#how-an-app-gets-its-engines)): nothing is downloaded, and nothing
is read from beside the app. The host keeps the one it
was sent as `${PERCH_HOME:-~/.perch}/bin/perchd-<build>`, where `<build>`
is the first 12 hex digits of the binary's SHA-256, `1a2b3c4d5e6f`. So the
same engine is never sent twice, whichever app has it, and a different one
never takes its name. There are no versions in it: two apps carry the same
engine or they do not.

- The host's login shell runs a short `sh` script, which is given the
  build of each engine the app carries. It starts the bridge of the one
  for what the host is, which starts the engine if none is running and
  joins it if one is. With that build not there, it writes
  `perch-install: <uname -s> <uname -m>` to stderr and exits 97.
- On that line the app sends the engine and connects again at once,
  outside the waits between tries. The strip at the foot of the window
  says "Sending perchd 0.9.0 to <host>…" meanwhile, with how far it has
  got under it. Linux and macOS, on `x86_64` or on `aarch64` or `arm64`,
  have a `perchd`; any other host says what it is and to put `perchd`
  there by hand and set `[remote] command`.
- A second ssh, with the same options, sends the binary to a script on the
  host that writes it to a temporary file in `bin` (the directory and the
  file owner-only), takes its SHA-256, and puts it in place only when
  that is the one the app said: an upload cut short, or changed on the
  way, is never run. The host needs `sha256sum`, `shasum` or `openssl`
  for it. A build that is already there is left as it is, so two apps
  sending the same one at once do not put a file under one another. The
  temporary file is removed when anything fails, the ssh session ending
  under it included.
- A send that goes through, after which the host still says it has no
  `perchd`, stops the tries and says so rather than sending again: most
  often a login shell there that sets another `PERCH_HOME`. An upload
  that fails is tried again with the next try.
- An app with no `perchd` for the host, and no source to build one
  from, says so and stops the tries.

#### How an app gets its engines

Building the app builds its engines: `crates/perch-app/build.rs` builds
`perchd` for Linux and macOS, on x86_64 and arm64, with the app's own
profile, gzips each into the app's binary and records its SHA-256, so the
app can name its builds without unpacking them. The app itself is built
for the machine it is built on only. This is so of every build, `cargo
run` as much as a release's.

- A Linux `perchd` is built against musl through `cargo zigbuild`, zig
  being the C compiler and linker for it from any machine: it needs zig
  and cargo-zigbuild (`brew install zig`, `cargo install cargo-zigbuild`).
  On Linux with neither, this machine's own processor's is built with
  `musl-gcc` when that is installed.
- A macOS `perchd` is built by the Rust toolchain on a Mac, for either
  processor. Anywhere else it needs Apple's SDK: it is built through zig
  when `SDKROOT` names one, and is left out otherwise.
- `rust-toolchain.toml` names the four targets, so rustup installs them.
- One whose tools are missing is left out, with a warning from the build
  that says what to install, and the app carries the rest. One that does
  not compile fails the app's build.
- They are built in `engines` under cargo's target directory, apart from
  the app, and built again when anything `perchd` is made from changes.
- With `PERCH_ENGINES` set to a directory, by its whole path, nothing is
  built: the four `perchd-<linux|macos>-<x86_64|arm64>.tar.gz` there are
  taken, and one missing fails the build. A release is built so, each
  engine on a runner of its own system. With `PERCH_ENGINES=none` the app
  carries none and nothing is built, for a quick build of the app alone.
  Under clippy and rust-analyzer nothing is built either.

For a host the app carries no engine for, an app whose source is still
where it was built builds `perchd` from it then: on the host's
`perch-install` line it builds for what the host is, and uploads it as a
carried one is, under its own hash. The strip says "Sending perchd 0.9.0
(built from source) to <host>…" through both. A Linux host's is built with
`cargo zigbuild` against musl, and a Mac's with `cargo build` from a Mac;
when a tool is missing the strip says which, with the command. Each
launch builds again, which cargo makes quick when nothing changed, and
asks the host whether it has that build before uploading it.

`[remote] command`, when set, is run instead, and nothing is sent.

Each bridge the app put there removes the builds beside it that nothing
has used in a week (see [perchd](#perchd)). A build an app still connects
with is used each time it does, so two apps of different builds each keep
theirs on the host.

### When the engine is another build

The app on the Mac looks for its own updates, as it does with no host:
Check for Updates… and Restart to update work while disconnected, and
never go to the host's engine. It asks GitHub with no token, since the
host has the token and the repository is public.

An updated app carries another engine, so its next connection sends the
new `perchd`, which goes beside the old. The engine already running on the
host may be the old build still, though one nobody has used for 15 minutes
has stopped by then (see [What runs where](#what-runs-where)) and the new
one simply starts. One still running is another build than the app's, as
is a dev build rebuilt, or another Mac's app of another version. Builds
are never ordered, only told apart, and the app wants its own:

- No runs and no other client connected: the app hangs up and connects
  again with `perchd bridge --replace-if-idle`, which stops that engine
  and starts the app's. The strip says "Restarting the engine on <host>
  to run perchd <build>…" meanwhile, and the log says so. It does
  this once a connection: an engine still another build after it is left
  running.
- Busy, with runs or another Mac connected: the app leaves it running. When
  it speaks the app's protocol, the app connects to it as usual and
  the strip says "<host>'s engine is perchd 0.8.0 (1a2b3c4d5e6f); restart
  it to run this app's, perchd 0.9.0 (2 agent runs would be stopped)",
  with a Restart button. When it
  doesn't, the strip says the same without a connection, with Restart and
  Reconnect. Restart connects with `perchd bridge --replace`, which stops
  the engine whatever it is doing: its runs are recorded Interrupted,
  as when the app quits, and other Macs connected to it reconnect to the
  new one. An engine that comes into use between the app's look and its
  `--replace-if-idle` is left running, and the strip offers the restart;
  the app connects to it when it speaks the app's protocol.
  The counts the strip names are from when the app connected, and are not
  brought up to date while it stays connected.
- Two Macs with different builds, at the same host, take turns: the first
  to connect has its engine, the second joins it while it speaks the
  second's protocol, and each replaces the other's only when the other's
  has nothing running and nobody connected. An app older than the engine
  it replaces can find `perch.db` moved on by the newer one, which its
  own `perchd` does not start on: the strip says so, and to update Perch
  on that Mac.

With `[remote] command` set none of this happens: the app uses
whatever engine the command reaches.

The app's first message, and the engine's answer, are frozen as of
protocol 5: later versions only add fields that may be left out. So an
engine of any later protocol still says who it is when it refuses the app,
and still hears a request to stop. An engine from before protocol 5 says
nothing of itself and can't be asked to stop; the strip says to stop its
`perchd serve` on the host and reconnect.

`perch --connect [host]` does what the app does to connect, with no
window: it sends `perchd` if the host needs it, replaces an engine of
another build with nothing running, prints the engine's version, protocol, build,
host name, home and data folder, how many other clients it has and how
many runs, and exits 0. An engine of another build left running for its
runs is said after that, and then the host's checks, a line each,
with the command to run under each that failed (see [What the app
checks](#what-the-app-checks)). It waits up to 30 seconds for them, as an
engine that has just started looks after it has found the agents. It reads `[remote]` from the Mac's
config.toml, and `[remote] host` when no host is given. On failure it says
why and exits 1.

### What runs where

The host runs the engine: the GitHub client and its token, `perch.db`, the
checkouts and worktrees, the language servers, the agents and their approvals,
and its config.toml's settings. The Mac runs the windows, draws the code and
pictures, and keeps its own `[ui]`, `[display]`, `[keybindings]` and
`[remote]`, and where its windows were left, in its own Perch home.

With the Mac closed or asleep, the engine goes on for as long as it has
work: runs started from the Mac keep going, queued ones start as others
finish, and meanwhile the inbox is polled and checks are watched. Once it
has had no client connected and no run queued or under way for 15 minutes,
the engine the app started stops, as SIGTERM stops it, and nothing of
Perch's runs on the host. The app starts it again when it next connects:
reopening the app, or waking the Mac, catches every window up with what
happened, from GitHub and from what the engine recorded. A run counts
until it has ended, one waiting on an answer included, and so does a
repository still being cloned for Add repository. An engine started
by hand with `perchd serve` stays until it is stopped. A Work run that asks something while no Mac is connected waits for
an answer, up to eight hours by default; the card is there when a window
connects, and is answered from there as on the Mac. Stopping `perchd`
interrupts its runs, as quitting the app does.

### Connecting

Windows using the same SSH host share a connection. Perch starts the
local engine when a window first uses the local environment. To reach a
remote environment, Perch runs

```sh
ssh -T -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 \
    -o ServerAliveCountMax=3 -o ForwardAgent=no -o ForwardX11=no \
    -o ClearAllForwardings=yes -- <host> <command>
```

and talks to the engine on the host over ssh's stdin and stdout. The
command runs `~/.perch/bin/perchd-<build> bridge` (see [The app's own
engine](#the-apps-own-engine)), or `[remote] command` when the Mac's
config.toml sets one for its default host, in your login shell either way. ssh's own
config, keys and agent apply to logging in, but nothing is forwarded,
whatever the config says: not the agent, not X11, no ports. Nothing is
asked either, as there is no terminal: connect once with `ssh <host>` in a
terminal to accept the host's key, and keep a key with a passphrase in the
agent or the macOS keychain. The command is run in your login shell on the
host, bash, zsh or fish, so the engine has the `PATH` your profile sets and
finds `git`, `gh` and the agents where you do. What ssh says on stderr goes
to the log. `PERCH_SSH` in the app's environment names another ssh program
to run, everywhere the app runs one.

The host's engine keeps its items, runs and checkouts going with the app
closed. Paths the app shows are the host's, and `~` in them is the host's
home. What would open a path on the Mac reaches the host instead:

- Open in Terminal opens Terminal with `ssh -t -- <host> 'cd -- <path> && exec $SHELL -l'`.
- Reveal in Finder becomes Open in Editor: Zed, as `zed ssh://<host>/<path>`,
  when its `zed` command or the app is installed, else VS Code, as
  `code --remote ssh-remote+<host> <path>`. When neither can be run, the
  window says so.
- Only a full path is opened. Open in Terminal refuses a path with a
  quote, a backslash, a `!` or a control character, since the Mac's login
  shell reads the line and fish or csh quote differently from sh.
- Copy Path copies the host's path.

While the host can't be reached, a strip along the foot of each window using it
says so: "Disconnected from <host>. Reconnecting…", or "Can't connect to
<host>" before the first connection, with ssh's last line under it (an
unknown host, a refused key) and a Reconnect button. Perch tries again after
a second, then two, doubling to every 30 seconds; Reconnect gives up a try
under way and tries now. A try the engine has not answered in 30 seconds
is given up. A connection that is lost within a minute of being made does
not start the waits again from a second, so an engine that fails as it
starts is not restarted every second. The windows keep what they showed. A
command sent meanwhile, a click or a comment, is not done and not kept for
later: the window says "Not connected to <host>, so that wasn't done." What
a window asked for while the first try was under way is sent if it
connects, and said not done if it fails. A window losing or gaining the
keyboard, and a comment's picture, are not refused: the engine hears which
window has the keyboard once connected, and pictures that never came are
asked for again. When the connection is back, each window catches up on
everything (the rows, the inbox, the runs, the settings) and reads its item
again, or opens the one it was opening. An item showing is read again
under what shows, so text typed into its composer or review is kept.
Opening a notification, the Open box, a search result or Start work while
disconnected puts "The connection to <host> was lost before this opened.
Try it again." on the progress card. Losing the connection after asking,
before the engine answers, says instead that it may have been done and to
look for it in the list before trying again. Neither is done again by
itself, since Start work done twice makes two branches.

When the app and the engine speak different versions of the wire, the
engine refuses the app and Perch stops trying until you press Reconnect.
With the app's own `perchd`, the strip offers to restart the engine
(see [When the engine is another build](#when-the-engine-is-another-build)); with
`[remote] command`, it says which one to update.

In a remote window, repositories, agents and GitHub settings belong to the
host. Appearance, display and keyboard shortcuts
come from the app’s local configuration. **General > Configuration files**
shows where each file lives. If a file cannot be read, Perch keeps its
last settings and disables edits to that file. You can still switch the
window’s environment.

### Adding a repository on a host

Add repository… would offer the Mac's folders, which the host has none
of. With a host, wherever it is asked for, it opens Settings >
Repositories, where the last row is a box and an **Add** button in place
of the folder chooser. Type one of:

- A repository on github.com, as `owner/name` or its URL. The engine
  clones it on the host, with its own GitHub token, to `<name>` in the
  folder the **Clone into** row beside it names (`clone_root` in the
  host's config.toml, such as `"~/Code"`), or in the home directory
  there with none, and adds it. A folder of that name that is already a
  clone of it is used as it is; any other is left alone and named. The token is
  given to that one `git clone` and is not written into the clone, whose
  later fetches and pushes use the host's own git credentials, as `gh auth
  login` sets them up.
- A path on the host, starting with `/` or `~/`: a clone already there.

Enter or **Add** sends it; clicking away sends nothing. The app says a
named repository is being added, and that it was added, since a clone
can take a while. What was typed stays in the box until then, so a
refusal, which the popup explains, can be put right. A repository on a
GitHub Enterprise host is cloned by hand and added by its path.

### Limits

- With `[remote] command` set, nothing updates `perchd` on the host. When
  the app is updated and the wire changes, the strip says to update the
  engine; install the new release's `perchd` and restart it.
- Windows hosts are not supported.

## Development

```sh
cargo test --workspace
cargo run -p perch-app
```
