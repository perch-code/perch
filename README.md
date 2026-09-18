# Perch

A native macOS tool for reviewing code, written entirely in Rust with
[floem](https://github.com/lapce/floem).

The sidebar lists your projects, and under each the things being worked on:
a pull request someone opened, an issue, a branch of your own. Click one
and the view that fits it opens: a read-only editor over the diff with
rust-analyzer behind it, or the issue with its comments. Ask a coding
agent about the lines on screen and the answer stays there as a thread. What
needs looking at sits at the top, and every discussion is archived as
markdown that a coding agent can search. Nothing in Perch writes to
GitHub.

## What it does

**Projects and items**

- A project is a name, the repositories that belong to it, and the
  directories where you already work on them, declared in `config.toml`.
  A repository nobody declares gets a project named after it the first
  time something from it is opened.
- Under a project, an *item* is one row with up to three parts: a GitHub
  pull request or issue, a checkout on disk, and attention. A branch of
  your own becomes a pull request when GitHub opens one on it, and it is
  the same row before and after, with the same threads.
- **Priority** at the top lists items with attention newer than their Done
  mark, newest first, with why: review requested, mentioned, commented, CI
  failed. **Pinned** holds what you pinned. Then one section per project,
  or one per date, with a count of active items.
- Each row has Save and Done on hover and a menu: Open in GitHub, Finder or
  Terminal, Pin, Mark unread, Rename, Fork or Start work, Move to project,
  Archive, Delete. Every entry is on every row; the ones that cannot apply
  are greyed. The filter button narrows the list by status, kind, grouping
  and sort, and the search box matches titles, numbers, branches and
  project names.

**The inbox**

- Behind the bell, with a count: the notifications that are not about any
  item. A notification about an item becomes the item's attention instead
  and is acted on there.
- Lists your notification inbox, grouped by repository or date.
- Sidebar filters: Assigned, Participating, Mentioned, Team mentioned,
  Review requested, plus a per-repository filter with counts.
- All / Unread toggle, substring search over title and repository.
- Sort newest or oldest first.
- Issue and pull request state icons: open, closed, not planned, draft,
  merged, and closed.
- Caches the inbox in SQLite so a cold start paints before the network
  answers, and works offline against the last known state.
- Polls on GitHub's own `X-Poll-Interval`, with conditional requests so an
  unchanged inbox costs one cheap 304.

**Saved and Done**

- Bookmark a notification into Saved, or dismiss it into Done, from the row,
  from the keyboard, or from the header of a pull request you have finished
  reading.
- Check rows and act on them together, or dismiss a whole repository from its
  group header.
- A dismissed thread comes back to the inbox when somebody comments on it,
  the way it does on the web. A bookmark does not expire.
- Both flags are Perch's own and stay on this machine. See below.

**The review view**

- Click a pull request and Perch clones the repository, checks the head out
  on a branch of its own (`perch/pr-<n>`) in a worktree, and shows what
  changed. A second pull request from the same repository costs a fetch
  rather than a clone. Opening the same one again finds the same directory.
  A pull request whose head branch you already have checked out under a
  project root opens in that directory instead.
- A branch of your own opens the same way, diffed against the default
  branch, with the branch and the directory in the header and no pull
  request block. When the branch has nothing past its base the diff is
  empty and the rail shows the whole tree.
- The header names the checkout's directory. Click it to reveal it in the
  Finder, open a Terminal there, or copy the path.
- A rail of changed files, or the whole worktree, with change markers on
  files the pull request touched.
- A read-only editor with tree-sitter highlighting for Rust, TOML, Markdown,
  YAML, and JSON.
- The unified diff in place: added and modified lines tinted, removed lines
  shown above the code that replaced them, and a gutter carrying real file
  line numbers with change bars.
- Click a symbol and rust-analyzer says what it is, with go-to-definition
  from the same panel.

**Issues**

- Open an issue and Perch shows its body and comments, the labels and
  assignees, and under a divider the branches and pull requests it has
  started from it.
- Ask about it the way you ask about a pull request. The question runs in
  the repository's *home checkout*, a worktree on the default branch that
  Perch keeps up to date and never lists, so "where does this live" gets
  an answer from the code.
- Press Start work and Perch cuts `perch/issue-<n>` from the default
  branch, checks it out, and opens it as a branch item that points back at
  the issue. Nothing is assigned or commented on GitHub.

**Branches, roots, and checkouts**

- Declare `roots` on a project and every git worktree under them on a
  branch other than the default becomes an item, with lines added and
  removed against the base on the row. Perch never touches a directory it
  did not make: Archive and Delete on such a row act on the row. Switch a
  root to another branch and the row for the old one is archived until the
  branch is checked out again; remove the worktree and the row goes,
  unless it has threads.
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

- Opening a pull request also archives its discussion: the description,
  the conversation, each review and the line comments under it, as plain
  markdown under `~/.perch/knowledge/<project>/github/<owner>/<repo>/pulls/`.
  Every message carries its author, time and a permalink.
- A skill is written for Claude Code and Codex that says where the files are
  and how to search them, so an agent working in one of those repositories
  can answer "why was this done" from the thread rather than guessing.
- Ripgrep is the search. There is no index and no database: `raw/` holds one
  JSON record per line, and everything else is rendered from it.

**Threads**

- Select lines in the editor and press `a`, or click the `+` in the gutter,
  to ask a coding agent about them. The answer appears beside the code as
  a thread anchored to those lines, with a marker in the gutter, and takes
  follow-ups. Press `a` with no file open, or the `+` at the top of the
  column, to ask about the pull request as a whole.
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

## What it deliberately does not do

Nothing in Perch writes to GitHub. There is no mark as read and no
unsubscribe.

Saved and Done do not sync, in either direction. Dismissing a thread in Perch
leaves it in your inbox on github.com, and dismissing one on github.com does
not dismiss it here. This is not an oversight. The REST API can mark a thread
done, but nothing lists done threads back, and Saved has no endpoint at all,
so a Done view built on the API could only ever show what this copy of Perch
put there. It shows exactly that, and says so, rather than pretending to be
the same inbox you see in a browser.

## Keys

| Key | What it does |
|---|---|
| `e` | Mark done, or restore, the row under the pointer, in the sidebar or the inbox. |
| `s` | Save or unsave, the same way. |
| `Escape` | Drop the selection, or put away a rename or a confirmation. |
| `Cmd-\` | Show or hide the sidebar. |
| `Cmd-[` | Back from a review to wherever it was opened from. |
| `Cmd-1` | The sidebar's search box. |
| `Cmd-2` | The inbox. |
| `Cmd-W` | Close the open file. |
| `P` `U` `R` `F` `A` `D` | Inside an open row menu: Pin, Mark unread, Rename, Fork, Archive, Delete. |
| `a` | Ask about the selected lines, or the pull request. |
| `Cmd-Enter` | Send the question. |
| `Cmd-Shift-\` | Show or hide the thread column. |
| `Escape` | Close the composer. |

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
  config.toml        projects, token, agent, knowledge settings
  perch.db           items, projects, threads, marks, the inbox cache
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
token = "ghp_..."        # only when env vars and gh are unavailable
poll_interval_secs = 60  # a floor; X-Poll-Interval may raise it

[[project]]
name = "perch"
repos = ["criccomini/perch"]
roots = ["~/Code/perch"]         # optional; checkouts you already have

[[project]]
name = "slatedb"
repos = ["slatedb/slatedb", "slatedb/slatedb-go"]
```

`repos` says which GitHub repositories belong to a project; `roots` says
where you already work on them, and each root's worktrees become items.
A repository no project claims gets an implicit project named after it
when an item needs one, and declaring it later adopts the items.

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

```toml
[agent]
binary = "claude"              # optional; found on PATH by default
model = "sonnet"               # optional; what the picker starts on
effort = "medium"              # optional; low | medium | high | xhigh | max | ultra
models = ["fable", "opus", "sonnet", "haiku"]   # optional; what the picker offers

[agent.codex]
binary = "codex"               # optional; found on PATH by default
models = ["gpt-5.6-sol"]       # optional; empty, and Codex is not offered
```

The top-level keys mean Claude Code, so an older file keeps working. The
picker lists the Claude Code models, then Codex's under a line when there
are any. A model named under both goes to Claude Code.

`perch --ask owner/repo#N path:start-end "question"` runs one question
from a terminal and prints the prompt, every event, and the answer. Leave
the path out to ask about the pull request as a whole, or name an issue
to ask about it from the home checkout. `perch --items` prints the sidebar
as text, and `perch --scan` what the root scan finds.

## Knowledge

Discussion lands under `knowledge/` in the Perch home. Change it with
`knowledge.root`. Which project a repository's files go under follows the
same `[[project]]` list as the sidebar:

```toml
[knowledge]
root = "~/.perch/knowledge"   # optional; this is the default
skills = true                 # optional; write the two SKILL.md files
```

The layout, per project:

```
~/.perch/knowledge/
  index.md                          projects, their repositories, last updated
  slatedb/
    index.md                        this project's containers and how current
    raw/github/slatedb-slatedb.jsonl  one record per line; the truth
    github/slatedb/slatedb/pulls/812.md   rendered from raw
    github/slatedb/slatedb/issues/42.md   an issue, the same way
```

Everything outside `raw/` can be deleted and comes back the next time a
pull request is opened. Deleting `raw/` too only loses what has not been
opened since; a pull request is archived again whenever it is opened, and
re-opening one that has not changed writes nothing.

The skill goes to `~/.claude/skills/perch/SKILL.md` for Claude Code and
`~/.agents/skills/perch/SKILL.md` for Codex. It names every project the
store holds and the root, and tells the agent to start from `index.md` and
search with `rg`. Nothing else is needed to point an agent at the files;
ask it why a change was made in a checkout of one of those repositories.

Only pull requests opened in Perch are archived. There is no backfill and
no timer.

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

`perch --dump-file owner/repo#N path` renders a file the way the editor would,
gutter numbers and change bars included, as text.

Two environment variables open a pull request at launch instead of by
clicking, which is useful when iterating on the review view:

```bash
PERCH_OPEN_PR=slatedb/slatedb#2092 PERCH_OPEN_FILE=slatedb/src/ops.rs cargo run --release
```

`scripts/screenshot.sh out.png` captures the running window. It needs the
screen unlocked; macOS will not composite an app window behind the login
window.
