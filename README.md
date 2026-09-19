# Perch

A native macOS tool for reviewing code, written entirely in Rust with
[floem](https://github.com/lapce/floem).

The sidebar lists pull requests, issues, and branches together, with tabs
for All, Notifications, and Authored. Click one and the view that fits it
opens: a read-only editor over the diff with
rust-analyzer behind it, or the issue with its comments. Ask a coding
agent about the lines on screen and the answer stays there as a thread. What
needs looking at is marked, and every discussion is archived as markdown
that a coding agent can search. Nothing in Perch writes to GitHub.

## What it does

**Repositories and items**

- A repository is declared in `config.toml` as `[[repo]]`: its `github`,
  `owner/repo`, the `roots` where you already work on it, or both.
  Roots alone take the repository from their GitHub remote, or make a
  local repository when they have none. Declaring a GitHub repository
  lists it whole: every open pull request and issue is a row, kept current
  on every poll, and one that merges or closes says so. A repository
  nobody declares appears in the Repository filter the first time an item
  from it is opened.
- An *item* is one row with up to three parts: a GitHub pull request or
  issue, a checkout on disk, and attention. A
  branch of your own becomes a pull request when GitHub opens one on it,
  and it is the same row before and after, with the same threads. An item
  is in the repository its pull request, issue or checkout is in; there is
  nothing to move.
- The list is flat across repositories, with pinned items first, then the
  remaining items in the selected sort order. Every row's second line
  names its repository, such as `slatedb/slatedb #57 · branch`.
- **All**, **Notifications**, and **Authored** select the list's scope.
  Notifications shows items that need attention; Authored shows pull
  requests and issues opened by your signed-in GitHub account, even when
  their latest notification has a different reason. Repository, status,
  kind, saved state, reason, and search further narrow the selected tab.
  The tab and filter settings are remembered between launches.
- Attention comes from your notifications, polled on GitHub's own
  `X-Poll-Interval`. Every notification about an item becomes the item's
  attention, with its reason. The Notifications tab collects review
  requests, mentions, assignments, comments, and pushes to threads you
  subscribe to. The
  one exception is CI, which fetches the open pull request's checks
  again and is attention when they fail. One about anything that has no
  row is dropped. A row with attention has its title in bold and a dot at
  its end, next to the pin on pinned rows; Reason narrows to why, which is
  where to leave subscribed activity out if it is noise to you.
- Each row has Save and Done on hover and a menu: Open in GitHub, Finder or
  Terminal, Pin, Mark unread, Rename, Fork or Start work, Archive, Delete.
  Every entry is on every row; the ones that cannot apply are greyed. The
  filter button narrows the list by status, repository, kind, saved state
  and reason, and sets the sort and its direction, newest or oldest first.
  The search box matches titles, numbers, branches and repository names.
- Save and Done are marks on the item, set from the row, from the keyboard,
  or from the header of the open item. Done clears the item's attention,
  and the next notification that asks for you brings it back, the way a
  done thread comes back on the web; Save does not expire. Status set to
  Done lists what is done, and Saved only narrows the list, pinned items
  included, to what is saved. Both marks are Perch's own and stay on this
  machine. See below.

**The item view**

Every item opens the same way: a header, then two tabs, Conversation and
Files, shaped like a pull request's page on GitHub.

- The header has the state as a pill (Open, Draft, Merged, Closed; Not
  planned for an issue), the branches, the checkout's directory (click it
  to reveal it in the Finder, open a Terminal there, or copy the path),
  the lines added and removed, and a checks pill when the head has
  workflow runs: green, red, or a dot while they run, hover for the
  workflow that failed, click to open the run. Under the bar, a strip
  with the reviewers and their verdicts, the labels, the assignees, the
  milestone, the issues the pull request closes, and the branches and
  pull requests started from an issue. Every part is absent when empty.
- **Conversation** is one column of cards in time order: the description,
  each comment, each review with its line comments nested under it (the
  lines it was about, tinted as the diff tints them; click the path and
  Files opens there), the events (opened, merged, closed), and every
  thread you have had with an agent about the item. A GitHub card has a
  permalink and, on hover, Ask about this, which opens the composer with
  the comment quoted. The composer for asking about the item as a whole
  sits at the end of the column.
- A thread of your own never looks like a comment from GitHub: the card is
  on the page colour with a dashed border and a Local chip, and says which
  model and effort answered.
- **Files** is the review: a rail with the worktree as a tree, a
  read-only editor with tree-sitter highlighting for Rust, TOML, Markdown,
  YAML and JSON, the diff in place with real line numbers and change bars,
  and the thread column. The tree starts cut down to the changed files
  and the directories on the way to them, a run of lone directories
  folded into one row; the filter button at the right of the Viewed row
  turns that off and shows the whole worktree, with the changed files
  still marked. At the top of the rail a scope menu narrows the diff to
  one commit, to the uncommitted work (staged, unstaged, or both), or to
  what changed since you last looked; the rail, the tints, the churn and
  the tab's count all follow it. Each changed file has its status and
  churn at the end of its row, and a Viewed check, keyed on the file's
  content, so a push that changes the file clears it and one that leaves
  it alone does not; `}` and `{` step through the unviewed files. A
  toggle in the file tab bar switches between the stacked diff and a
  split one, base beside head, scrolling as one. Click a symbol and
  rust-analyzer says what it is; go-to-definition and find references
  are in the same panel, and references open in a panel under the
  editor.
- A changed file opens folded to its changes, the way git and GitHub show
  a diff: each change with three lines of context, and a divider across
  the code where the unchanged run between two is hidden, saying how many
  lines it hides; a run so short that a divider would be worse than the
  lines stays in view. Hover a divider and its gutter offers Up and Down,
  twenty lines at a time, or All when the run is forty lines or fewer,
  and Reset once some of it has been opened. What you open is a reveal,
  remembered per worktree and per version of the file, the way Viewed is:
  a push that changes the file opens it folded again, and checking the
  old head out brings the reveals back. A jump into a fold, from go to
  definition, a reference or a thread's label in the column, opens it
  three lines either side and is remembered the same way. Show whole, the
  tile left of Stacked and Split in the file tab bar, shows the worktree's
  files whole with no divider and is remembered per worktree, while
  Stacked and Split are per app: a reader folds one review and not
  another, but reads every review stacked or every one split. The status
  line says how many lines are hidden, and clicking the count is Show
  whole. A file outside the changed list has nothing to fold around and
  opens whole.
- Every thread anchored in the open file is drawn in the code under its
  lines, in the stacked layout and across both sides of the split: your
  own with the dashed face Conversation gives them, GitHub's review
  threads with the solid one, so neither can be taken for the other. The
  lines a thread is about are never folded; it keeps them in view the way
  a change does. A chevron at the card's left collapses it to one row,
  who wrote it, its first line and how many messages follow, and a thread
  that is Outdated starts collapsed; the marker in the gutter, solid for
  yours and hollow for GitHub's, scrolls the card into view. The composer
  opened with `+` or `a` is a card under the selected lines, and a
  follow-up is written in the thread's card. A card longer than forty
  rows scrolls inside itself, so a long conversation cannot push the code
  off the screen. The column stays as the list of every thread in the
  worktree, the ones in other files included, and does not open itself
  for a thread drawn in the code; `Cmd-Shift-\` shows it. A GitHub card
  has no resolved state, because the REST payload carries none, so it
  starts open.
- An issue has Conversation alone, until Start work gives it a checkout. A
  branch of your own has a Conversation of its threads and a Files of its
  diff against its base.
- Click a pull request and Perch clones the repository, checks the head out
  on a branch of its own (`perch/pr-<n>`) in a worktree, and shows what
  changed. A second pull request from the same repository costs a fetch
  rather than a clone. Opening the same one again finds the same directory.
  A pull request whose head branch you already have checked out under a
  root you declared, or on a branch Perch cut, opens in that checkout
  instead.

**Issues**

- Open an issue and Perch shows its body and comments as Conversation,
  with the labels, assignees, and the branches and pull requests started
  from it in the strip.
- Ask about it the way you ask about a pull request. The question runs in
  the repository's *home checkout*, a worktree on the default branch that
  Perch keeps up to date and never lists, so "where does this live" gets
  an answer from the code.
- Press Start work and Perch cuts `perch/issue-<n>` from the default
  branch, checks it out, and opens it as a branch item that points back at
  the issue. Nothing is assigned or commented on GitHub.

**Branches, roots, and checkouts**

- Declare `roots` on a repository and every git worktree under them on a
  branch other than its base becomes an item, with lines added and
  removed since it left its base on the row. The count is taken against
  the base as the remote has it for a GitHub root, `upstream/main` say,
  and against the base branch itself for a local one, or for a remote
  that has not been fetched yet; opening the row diffs against the same
  base. Perch never touches a directory it did not make: Archive and
  Delete on such a row act on the row. Switch a root to another branch
  and the row for the old one is archived until the branch is checked out
  again; remove the worktree and the row goes, unless it has threads.
- A pull request opened from a branch Perch cut, or from a root, is read
  in that checkout: nothing is reset, and if the pull request has moved
  past what is there the header says so. Only `perch/pr-N` checkouts follow
  the pull request's head.
- Fork cuts `<branch>-2` from an item's head into a new item, with an empty
  diff against the source, so what is shown is what was done on top.
- On every refresh Perch asks GitHub, conditionally, whether a pull request
  has been opened on each branch without one, and attaches it to the row.
- Archive an item and an owned checkout's directory goes while the branch,
  the row and the threads stay, so restoring it is a checkout rather than
  a clone. A checkout with uncommitted changes refuses to be archived, and
  says so in the status line. Delete removes the row and its threads, and
  for an owned checkout the directory and the branch. Owned checkouts not
  opened for a fortnight are archived on launch; nothing is deleted on a
  timer.
- If a pull request moves on while there are uncommitted changes in its
  checkout, Perch leaves them alone, shows what is on disk, and says so in
  the header. Clean checkouts follow the pull request.

**Knowledge**

- Every discussion is archived as plain markdown under
  `~/.perch/knowledge/<owner>/<repo>/github/`: a pull request's
  description, conversation, each review and the line comments under it,
  and an issue with its comments. Opening one archives it, and a declared
  GitHub repository is archived whole in the background, open items as
  they move and closed history oldest first, so once the backfill has run
  the archive covers the whole repository. Every message carries its
  author, time and a permalink.
- A skill is written for Claude Code and Codex that says where the files are
  and how to search them, so an agent working in one of those repositories
  can answer "why was this done" from the thread rather than guessing;
  once the backfill has run, that is anything the repository has ever
  discussed.
- Ripgrep is the search. There is no index and no database: `raw/` holds one
  JSON record per line, and everything else is rendered from it.

**Threads**

- Select lines in the editor and press `a`, or click the `+` in the gutter,
  to ask a coding agent about them. The answer appears under those lines
  as a thread, with a marker in the gutter, and takes follow-ups. Press
  `a` with no file open, or the `+` at the top of the column, to ask about
  the pull request as a whole.
- Each question picks its model and effort. The model picks the agent:
  Claude Code, or Codex when its models are configured. The thread shows
  what the agent did, one line per file read or command run, then its
  answer, then what it cost and how long it took. A run can be stopped.
- The agent is read-only by construction. Claude Code may read files,
  search, and run `git diff`, `git log`, `git show`, `git blame` and `rg`;
  edits are denied, and a denial shows as a step line. Codex runs under
  its own read-only sandbox. Either runs in the worktree, as a child
  process of Perch, on your own login.
- Threads belong to the item. Archiving keeps them; deleting removes them.
  When the pull request moves on and the anchored file changes, the thread
  says Outdated.
- A thread shows under its lines and in the column on Files, and as a
  card on Conversation. They are one thread; a follow-up from either
  shows in both. An answered thread is filed under knowledge beside the
  item's discussion, marked as yours and not GitHub's.

## What it deliberately does not do

Nothing in Perch writes to GitHub. There is no mark as read and no
unsubscribe.

There is no inbox, because every open pull request and issue in a declared
repository is already a row and a notification is a reason on one of them;
what a list of notifications would add is subscribed activity in
repositories you did not declare, CI on default branches, discussions,
releases and security alerts, and none of that is what Perch is for.

Saved and Done do not sync, in either direction. Marking an item done in
Perch leaves its notification where it is on github.com, and marking the
thread done there does not mark the item done here. This is not an
oversight. The REST API can mark a thread done, but nothing lists done
threads back, and Saved has no endpoint at all, so a Done status built on
the API could only ever show what this copy of Perch put there. It shows
exactly that, and says so, rather than pretending to be the same list you
see in a browser.

## Keys

| Key | What it does |
|---|---|
| `e` | Mark done, or restore, the row under the pointer, or else the open item. |
| `s` | Save or unsave, the same way. |
| `Escape` | Put away a rename or a confirmation. |
| `Cmd-\` | Show or hide the sidebar. |
| `Cmd-[` | Back from an item to the sidebar. |
| `Cmd-1` | The sidebar's search box. |
| `Cmd-Shift-1` `Cmd-Shift-2` | Conversation, Files. |
| `Cmd-W` | Close the open file. |
| `P` `U` `R` `F` `A` `D` | Inside an open row menu: Pin, Mark unread, Rename, Fork, Archive, Delete. |
| `a` | Ask about the selected lines on Files, or about the item on Conversation. |
| `Cmd-Enter` | Send the question. |
| `Cmd-Shift-\` | Show or hide the thread column. |
| `v` | Mark the open file viewed, or not. |
| `}` `{` | The next and previous unviewed file. |
| `]` `[` | The next and previous change in the open file. |
| `Shift-F12` | Find references to the symbol at the cursor. |
| `Escape` | Close the composer, or the references panel. |

`e` and `s` stand down while the search box has focus.

## Requirements

- macOS
- Rust 1.97 (pinned in `rust-toolchain.toml`)
- `git` on PATH
- `rust-analyzer` on PATH for Rust intelligence (optional; the editor works
  without it)
- A GitHub token with the `notifications` scope, or `Notifications: read` on
  a fine-grained token

## Token

Perch looks for a token in this order and takes the first hit:

1. `GITHUB_TOKEN`
2. `GH_TOKEN`
3. `gh auth token`
4. `token` in `config.toml`

A GUI app launched from Finder does not inherit your shell environment, so the
`gh` step is what makes a double-click launch work. If you have the GitHub CLI
set up, there is nothing else to do:

```bash
gh auth login
```

## Build and run

```bash
cargo run --release
```

The Dock icon is embedded in the binary, including when using `cargo run`.
To build a Finder-launchable macOS app with the same icon:

```bash
scripts/bundle.sh
open target/Perch.app
```

The icon artwork is in `assets/perch.svg`, drawn with editable vector paths
and gradients. Icon generation requires `rsvg-convert` (`brew install librsvg`).
Run `scripts/build-icon.sh` after changing it to regenerate `assets/Perch.icns`
at all macOS icon sizes, then rebuild the app.

## Where everything lives

Everything Perch keeps is under one directory, `$PERCH_HOME` if set, else
`~/.perch`:

```
~/.perch/
  config.toml        repositories, token, agent, knowledge settings
  perch.db           items, threads, marks, reveals, where the backfill is
  avatars/
  repos/             one bare clone per repository
  worktrees/         one checkout per branch Perch made, <owner>__<repo>__<branch>
  homes/             one checkout per repository on its default branch
  knowledge/         the archived discussion, as markdown
```

The worktrees are ordinary git worktrees of the bare clone: `git status`,
`git diff` and a shell all work in them. Archiving from Perch is the safe
way to reclaim the space, since it checks for uncommitted work first and
keeps the branch; a directory removed by hand simply shows as archived on
the next launch. Deleting the whole directory costs a cold start, every
mark and thread, and every checkout; marks and threads are the things
GitHub cannot give back.

The database carries a schema version. One from an older build is
recreated on launch, with a notice; the clones stay and are reused.

## Configuration

Optional, at `~/.perch/config.toml`:

```toml
token = "ghp_..."                # only when env vars and gh are unavailable
poll_interval_secs = 60          # a floor; X-Poll-Interval may raise it
github_host = "ghe.acme.com"     # optional; an Enterprise host that counts as GitHub

[[repo]]
github = "criccomini/perch"
name = "perch"                   # optional; what the sidebar shows
roots = ["~/Code/perch"]         # optional; checkouts you already have
base = "main"                    # optional; when no remote HEAD says

[[repo]]
github = "slatedb/slatedb"

[[repo]]
name = "notes"
roots = ["~/Code/notes"]         # no GitHub remote: a local repository
```

`github` is the repository on GitHub, `owner/repo`, compared without
regard to case. `name` is what the sidebar shows, and defaults to the
repository half of `github`, `perch` above, or to the first root's
directory name. `roots` says where you already work on it, and each
root's worktrees become items. A root belongs to the repository its
GitHub remote names, `upstream` first, then `origin`, then any other, so
a checkout of each repository goes under its own `[[repo]]`; a root whose
remote names some other repository is skipped, and the status line says
so. `base` is the base branch, for when no remote HEAD says which it is;
without it Perch takes `main` or `master`, whichever exists. An entry
with neither `github` nor `roots` has nothing to list and nothing to
scan, and is skipped with a notice. `github_host`, at the top level,
names a GitHub Enterprise host whose remotes count as GitHub beside
`github.com`.

An entry with `roots` and no `github` takes its repository from the
first root: a GitHub remote makes it that repository, and none makes it
a local one. A local repository is `local/<name>`, so `local` is a
reserved owner and `github = "local/..."` is refused. Its branches are
items, its threads are filed under `knowledge/local/<name>/`, and
nothing about it is asked of GitHub. Changing its `name` moves where new
knowledge is filed and does not move what is already there.

A checkout whose `origin` is your fork and whose `upstream` is the
repository belongs to the repository, since the scan takes `upstream`
first. Declare `github` as the upstream repository, or leave `github`
out and let the scan decide, and list the checkout under `roots`. Its
branches are rows of the repository, counted and diffed against
`upstream`'s base once `upstream` has been fetched, and a pull request
opened from one of them is looked up in the repository with your fork as
its head, and attaches to the branch's row. A file that still declares
the fork itself, `github = "me/widgets"`, gets a notice that the root's
remote names the upstream, and the root is skipped until the entry says
so too.

Declaring a repository puts it in the sidebar in config order and gives
its `roots` a home. One on GitHub is also listed whole: on every poll
its open pull requests and issues become rows, with nothing marking them
new, a row whose pull request merged or closed says so, and its history
is archived under knowledge in the background. A repository you did not
declare shows only what you opened, and its section looks the same.

An older file that says `name = "owner/repo"` with no `github` still
loads, the name read as `github`, with a notice. A file that still
declares `[[project]]` is not loaded, and the notice says to write
`[[repo]]` instead.

## Threads

A question goes to one of two agents, and the model chosen in the
composer decides which.

Claude Code runs as `claude -p` in the worktree with `--output-format
stream-json`, an allowed-tool list that permits only reading and a few
git commands, and the writing tools explicitly denied. Nothing is passed
that skips permissions. A follow-up resumes the agent's own session with
`--resume`, so Perch never resends the conversation.

Codex runs as `codex exec --json` in the worktree under its read-only
sandbox (`-s read-only`; `sandbox_mode` on resume). The framing goes in
as `developer_instructions`. A follow-up resumes the thread by id with
the model repeated, since Codex would otherwise fall back to its default.
Codex reports no cost, so the thread shows only the time.

A follow-up resumes the last finished run only when the same agent
answered it. Switching agents in a thread starts over, with the earlier
messages folded into the prompt.

Effort is `low`, `medium`, `high`, `xhigh`, `max` or `ultra`. The rungs
below `ultra` pass straight through to both agents. `ultra` lets the
agent delegate. On Claude Code it is `--effort ultracode`: xhigh plus the
Workflow and Agent tools, whose subagents inherit the read-only tool list.
On Codex it is `model_reasoning_effort=ultra`; the helpers it spawns do
not appear in the thread, only the parent's commands and answer. Only
some models support it on either side.

Nothing needs configuring for either. At startup Perch looks for
`claude` and `codex` on `PATH` and in the directories they install into,
and offers whichever it finds. Claude Code's models are its aliases.
Codex's come from its own catalog, `codex debug models`, which also says
which effort levels each takes: a level a model lacks is greyed out in
the picker, and picking such a model drops the effort to the highest it
has, since Codex would otherwise run the lower level quietly. An agent
whose binary is missing is not offered; with neither, the window says so.

```toml
[agent]
binary = "claude"              # optional; found on PATH by default
model = "sonnet"               # optional; what the picker starts on
effort = "medium"              # optional; low | medium | high | xhigh | max | ultra
models = ["fable", "opus", "sonnet", "haiku"]   # optional; what the picker offers

[agent.codex]
binary = "codex"               # optional; found on PATH by default
models = ["gpt-6-astra"]       # optional; pins the list instead of asking Codex
```

The top-level keys mean Claude Code, so an older file keeps working. The
picker lists the Claude Code models, then Codex's under a line when there
are any. A model named under both goes to Claude Code. If the catalog
cannot be read, the one model Codex itself is set to run is offered.

`perch --ask owner/repo#N path:start-end "question"` runs one question
from a terminal and prints the prompt, every event, and the answer. Leave
the path out to ask about the pull request as a whole, or name an issue
to ask about it from the home checkout. `perch --items` prints the sidebar
as text, under the filter menu's settings when given them as
`"scope=notifications;repository=slatedb/slatedb;reason=mention"`.
`perch --scan` prints what the root scan finds.

## Knowledge

Discussion lands under `knowledge/` in the Perch home, one directory per
repository, `owner/repo`, whether or not the repository is declared, and
`local/<name>` for a local one. Change the root with `knowledge.root`:

```toml
[knowledge]
root = "~/.perch/knowledge"   # optional; this is the default
skills = true                 # optional; write the two SKILL.md files
```

The layout, per repository:

```
~/.perch/knowledge/
  index.md                          repositories, their directories, last updated
  slatedb/slatedb/
    index.md                        this repository's containers and how current
    raw/github.jsonl                one record per line; the truth
    raw/perch.jsonl                 your own threads, the same way
    github/pulls/812.md             rendered from raw
    github/issues/42.md             an issue, the same way
    perch/pulls/812.md              what you asked an agent about 812
    perch/branches/fence.md         what you asked about a branch with no pull request
```

The root `index.md` is a table of every repository the store holds and
the directory it is under, so an agent can go from the checkout's GitHub
remote, `upstream` before `origin`, or from `local/<name>` for a local
repository, to the right files without a lookup. Each repository's
`index.md` lists its containers and how current each is.

The `perch/` files are your own threads: each question, its answer, the
lines it was about and the model that answered, with front matter that
says `source: perch`. They are kept apart from the `github/` files so
nothing you asked on this machine can be taken for something said in
public, and the skill tells the agent which is which.

Every declared GitHub repository is archived whole while Perch runs. After each
poll a background task archives the open items whose discussion has moved
since it was last kept, newest first, then walks the repository's closed
pull requests and issues oldest first, a page at a time. It keeps its
place in `perch.db`, so a restart goes on from the page it was on, and
once the walk has reached the end it picks up only what has changed
since. The list is read by page number, so a closed pull request or
issue that is commented on or relabelled while the walk is past it moves
to the end of the list and shifts the rest back one, and the walk can
miss one entry; it is archived when it next moves or is opened in Perch.
It paces itself by GitHub's rate limit: with fewer than a thousand
requests left it waits for the reset, and neither the poll nor anything
you do is held by it. The status bar shows how far it is, "Archiving
slatedb, 312 of about 2,140", the repository by the name the sidebar
shows, and the line goes when every repository is caught up.
`perch --backfill <owner/repo> [--limit n]` runs one round for a
repository in the foreground, printing each discussion as it is archived
with the requests left after it, and says whether the repository is
caught up; `perch --sync <owner/repo>` runs one list pass and prints each
row it added, updated or closed, or "unchanged".

Everything outside `raw/` is rendered from it and comes back as
discussions are archived. Deleting `raw/` too loses the closed history,
since the backfill keeps its place in `perch.db` and does not walk again
on its own; what is open is archived again as it moves, a pull request is
archived again whenever it is opened, and re-opening one that has not
changed writes nothing.

A knowledge directory written before this layout has `<project>/raw/`;
delete those project directories, and the files come back under
`<owner>/<repo>/` as the backfill and your opens archive them again.
Perch neither reads nor migrates the old layout.

The skill goes to `~/.claude/skills/perch/SKILL.md` for Claude Code and
`~/.agents/skills/perch/SKILL.md` for Codex. It names every repository
the store holds and the root, tells the agent to start from `index.md`
and search with `rg`, and says what the archive covers: a declared
repository whole once the backfill has run, and `perch --backfill` is how
to see whether it has. Nothing else is needed to point an agent at the
files; ask it why a change was made in a checkout of one of those
repositories.

## Layout

| Crate | Contents |
|---|---|
| `perch-github` | Model, REST client, SQLite cache. |
| `perch-git` | Clone, fetch, worktree, diff, tree listing. |
| `perch-knowledge` | Discussion records, raw JSONL, rendered markdown, the skill. |
| `perch-agent` | Claude Code and Codex as child processes: the command lines, an event parser for each, spawn and cancel. |
| `perch-syntax` | tree-sitter highlighting. |
| `perch-lsp` | Language server client. |
| `perch-app` | Floem UI and the background worker. Binary target `perch`. |

None of the first four depends on floem, or on each other.

The split is deliberate: data and IO stay testable without a window, and stay
reusable when the review features land.

## Development

```bash
cargo test
cargo clippy --all-targets
```

Three example binaries check the layers below the window:

```bash
cargo run -p perch-github --example dump
cargo run -p perch-git --example checkout -- slatedb/slatedb#2091
cargo run -p perch-lsp --example hover -- <worktree> src/lib.rs 10 4
```

`perch --dump-file owner/repo#N path` renders a file the way the editor
would, as text: gutter numbers and change bars, a divider row for each run
folded away, and a spacer row for each row a thread's card takes, under
the reveals and Show whole the cache holds for the item. `--whole` renders
the file whole, with nothing folded and no card; `--split` renders both
sides of the split one after the other and ends in their row counts, so
the two being the same height can be checked by eye and by test. `perch
--timeline <item-id>` prints an item's Conversation one card per line,
and `perch --scope <item-id> <scope>` prints the changed list under a
scope (`branch`, `commit:<sha>`, `since:<sha>`, `uncommitted`, `staged`,
`unstaged`) with each file's viewed mark; `perch --items` names the ids.

Two environment variables open a pull request at launch instead of by
clicking, which is useful when iterating on the review view:

```bash
PERCH_OPEN_PR=slatedb/slatedb#2092 PERCH_OPEN_FILE=slatedb/src/ops.rs cargo run --release
```

`scripts/screenshot.sh out.png` captures the running window. It needs the
screen unlocked; macOS will not composite an app window behind the login
window.
