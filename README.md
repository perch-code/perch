# Perch

A native, read-only GitHub notifications inbox for macOS, written entirely in
Rust with [floem](https://github.com/lapce/floem).

Perch is the first milestone of a larger code review tool. Today it does one
thing: show your GitHub notifications the way the web inbox does, with the
same filters, sorting, grouping, and search, and nothing that mutates state.

## What it does

**The inbox**

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
- The header names the worktree's directory. Click it to reveal it in the
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

**Worktrees**

- The second entry in the rail lists every checkout Perch has made, active
  and archived, by repository, with the pull request's state and when it
  was last opened.
- Archive a worktree and the directory goes while the branch and the record
  stay, so restoring it is a checkout rather than a clone. A worktree with
  uncommitted changes refuses to be archived, and says so in the status
  line. Forget an archived worktree and the branch goes too.
- Anything not opened for a fortnight is archived on launch. Nothing is
  ever deleted on a timer.
- A worktree is somewhere work can happen. If the pull request moves on
  while there are uncommitted changes in it, Perch leaves them alone, shows
  what is on disk, and says so in the header. Clean worktrees follow the
  pull request.
- A worktree opens without a network. The list paints from a snapshot, and
  the review shows the checkout as it was last left.

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
  to ask Claude Code about them. The answer appears beside the code as a
  thread anchored to those lines, with a marker in the gutter, and takes
  follow-ups. Press `a` with no file open, or the `+` at the top of the
  column, to ask about the pull request as a whole.
- Each question picks its model and effort. The thread shows what the
  agent did, one line per file read or command run, then its answer, then
  what it cost and how long it took. A run can be stopped.
- The agent is read-only by construction: it may read files, search, and
  run `git diff`, `git log`, `git show`, `git blame` and `rg`. Edits are
  denied, and a denial shows as a step line. It runs in the worktree, as a
  child process of Perch, on your own Claude Code login.
- Threads live in `perch.db` beside the worktree record. Archiving a
  worktree keeps them; forgetting it removes them. When the pull request
  moves on and the anchored file changes, the thread says Outdated.

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
| `e` | Mark done, or restore. The selection, or the row under the pointer. |
| `s` | Save or unsave, the same way. |
| `Escape` | Drop the selection. |
| `Cmd-\` | Show or hide the drawer. |
| `Cmd-[` | Back from a pull request to wherever it was opened from. |
| `Cmd-1` | Notifications. |
| `Cmd-2` | Worktrees. |
| `Cmd-W` | Close the open file. |
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

## Configuration

Optional, at
`~/Library/Application Support/com.perch.perch/config.toml`:

```toml
token = "ghp_..."        # only when env vars and gh are unavailable
poll_interval_secs = 60  # a floor; X-Poll-Interval may raise it
```

Cache and avatars live beside it, and so do your Saved and Done marks and
the record of your worktrees. Deleting that directory costs a cold start,
every mark you have made, and every worktree; marks are the one thing
GitHub cannot give back.

Clones live under `repos/` in that directory, one bare clone per repository,
and worktrees under `worktrees/`, one directory per branch, named
`<owner>__<repo>__perch-pr-<n>`. They are ordinary git worktrees of the bare
clone: `git status`, `git diff` and a shell all work in them. Archiving from
Perch is the safe way to reclaim the space, since it checks for uncommitted
work first and keeps the branch; a directory removed by hand simply shows
as archived on the next launch.

## Threads

A question runs `claude -p` in the worktree with `--output-format
stream-json`, an allowed-tool list that permits only reading and a few
git commands, and the writing tools explicitly denied. Nothing is passed
that skips permissions. A follow-up resumes the agent's own session with
`--resume`, so Perch never resends the conversation.

```toml
[agent]
binary = "claude"              # optional; found on PATH by default
model = "sonnet"               # optional; what the picker starts on
effort = "medium"              # optional; low | medium | high | xhigh | max
models = ["sonnet", "opus"]    # optional; what the picker offers
```

`perch --ask owner/repo#N path:start-end "question"` runs one question
from a terminal and prints the prompt, every event, and the answer. Leave
the path out to ask about the pull request as a whole.

## Knowledge

Discussion lands under `~/.perch/knowledge`, outside the app's own data
directory, because agents have to be told the path and it has to survive
a reinstall. Change it with `knowledge.root`. A repository belongs to the
project named for it unless `config.toml` says otherwise:

```toml
[knowledge]
root = "~/.perch/knowledge"   # optional; this is the default
skills = true                 # optional; write the two SKILL.md files

[[knowledge.project]]
name = "slatedb"
repos = ["slatedb/slatedb", "slatedb/slatedb-go"]
```

The layout, per project:

```
~/.perch/knowledge/
  index.md                          projects, their repositories, last updated
  slatedb/
    index.md                        this project's containers and how current
    raw/github/slatedb-slatedb.jsonl  one record per line; the truth
    github/slatedb/slatedb/pulls/812.md   rendered from raw
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
| `perch-agent` | Claude Code as a child process: the command line, the event parser, spawn and cancel. |
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
