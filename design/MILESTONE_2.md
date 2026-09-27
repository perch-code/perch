# Perch: Milestone 2, the read-only editor

## Goal

Click a pull request in the inbox and land in a read-only editor over a real
checkout of that PR. A left rail lists the changed files by default and can
toggle to the full file tree. The editor highlights code with tree-sitter,
shows the PR's diff in the gutter, and answers hover and go-to-definition
from rust-analyzer.

This is the spike that decides whether the architecture holds. Floem's
editor crate, an in-process LSP client, and a per-PR worktree have never been
wired together in this codebase. Prove they can be, on slatedb, before
building review features on top.

Milestone 1 stays untouched except where noted under "Inbox changes".

## Non-goals

- Editing, saving, or any write to the working tree.
- Split (side-by-side) diff. Unified only.
- Comments, findings, or any GitHub write.
- Language servers other than rust-analyzer. The client is generic; only one
  server is wired up.
- LSP features beyond hover, go-to-definition, and diagnostics.
- Issues. Clicking an issue does nothing in this milestone.
- Private repositories the token cannot read. Fail with a clear message.
- Submodules. Clone without them.

## The flow

1. User clicks a PR row in the inbox.
2. Perch resolves owner, repo, number, head SHA, base SHA from the PR API.
3. If there is no bare clone for `owner/repo`, clone one. Show progress.
4. Fetch `refs/pull/N/head` into the bare clone. Also fetch the base SHA.
5. Create (or reuse) a worktree for the head SHA.
6. Compute the changed-file list by diffing base against head.
7. Replace the inbox with the review view. The rail shows changed files,
   the editor is empty until a file is clicked.
8. Start rust-analyzer against the worktree in the background. Hover works
   once it has indexed; the UI does not block on it.

Steps 3 through 6 run in the backend. The UI shows a progress card with the
current step, and the Inbox row in the sidebar stays clickable to abandon.

## Repository storage

```
~/Library/Application Support/com.perch.perch/
  repos/
    github.com/owner/repo.git/        bare clone, one per repository
  worktrees/
    owner__repo__<head-sha12>/        one per PR head
  perch.db                            unchanged from milestone 1
```

Bare clone plus worktrees is the model. Objects are shared, a second PR from
the same repo costs one fetch, and rust-analyzer can hold one worktree open
while another is being prepared.

Worktrees are not removed when the user leaves a PR. A background sweep at
startup removes any worktree not opened in the last 14 days. Bare clones are
never removed automatically.

## Git operations

Shell out to the `git` binary for anything that talks to the network or
mutates the store: clone, fetch, worktree add, worktree remove. Reasons:

- Authentication. The user's token needs to reach git. Pass it with
  `-c credential.helper=` and a `GIT_ASKPASS` script, or via
  `http.extraheader`. Do not embed the token in the remote URL; it ends up
  in `.git/config` and in error messages.
- Robustness. `git clone` handles every transport edge case libgit2 does
  not.
- It is what the user already has installed and trusts.

Use `gix` for reads: listing the tree at a commit, reading a blob, and the
base-vs-head diff. Use `git2` only if `gix` lacks something specific; note
it in the code if so.

Every subprocess gets a timeout. Clone gets ten minutes, fetch two, worktree
commands thirty seconds. Progress lines from `git clone --progress` are
parsed and forwarded to the UI as a percentage when git reports one.

The fetch refspec is `+refs/pull/N/head:refs/perch/pull/N/head`. Keeping the
ref in a `perch` namespace means it never collides with anything the user
does in their own clones.

## PR resolution

`GET /repos/{owner}/{repo}/pulls/{n}` gives head SHA, base SHA, head and base
ref names, title, author, draft, merged, and the clone URL. Cache it in
SQLite keyed by `(owner, repo, number, updated_at)`.

The changed-file list comes from the local diff of base SHA against head SHA,
not from the API. The API paginates at 3000 files and omits binary hunks;
the local diff is complete and free. Use the API's file list only if the
local diff fails, and log that it happened.

## The review view

Replaces the notification list inside the existing window. The milestone 1
sidebar stays on the left; the review view occupies where the toolbar and
list were.

```
+----------+------------------------------------------------------------+
| sidebar  | header: < back   slatedb/slatedb #2091  RFC 0035 ...        |
| (m1)     |         author avatar  main <- feature/incremental  +412 -87|
|          +--------------+---------------------------------------------+
|          | rail         | tabs: manifest.rs x | compactor.rs x        |
|          | [Changed|All]+---------------------------------------------+
|          |              |                                             |
|          | M src/a.rs   |   editor                                    |
|          | A src/b.rs   |                                             |
|          | D src/c.rs   |                                             |
|          | R d.rs->e.rs |                                             |
|          |              |                                             |
|          +--------------+---------------------------------------------+
|          | status: rust-analyzer indexing 42%  |  ln 120, col 8       |
+----------+------------------------------------------------------------+
```

### Header

Back button, `owner/repo #N`, title, author avatar and login, base and head
branch names, total additions and deletions, and a state badge (open, draft,
merged, closed) using the milestone 1 icons.

### Rail

Fixed at 280px, resizable later. A segmented control at the top: **Changed**
and **All files**.

**Changed** is a flat list sorted by path. Each row: a status glyph
(A green, M yellow, D red, R purple), the path with the directory portion
muted, and `+n -m` right-aligned. Renames show `old -> new`. Deleted files
open in the editor showing the base version, all lines marked deleted.

**All files** is a tree of the worktree. Directories collapse. Files that are
in the changed set carry the same status glyph so the diff is visible from
the tree too. Ignore `.git` and honour `.gitignore`.

Clicking either opens the file in the editor. The active file is highlighted
in whichever list is showing.

### Tabs

One per open file. Click to switch, `x` to close, middle-click to close.
Opening a file already open focuses its tab. Cap at twenty; opening a
twenty-first closes the least recently used unpinned tab. No pinning in this
milestone, so: closes the oldest.

### Editor

Floem's `TextEditor` from `floem-editor-core`, read-only.

- Tree-sitter highlighting for Rust, TOML, Markdown, YAML, JSON. Grammars
  compiled in. Any other extension renders as plain text.
- Line numbers in the gutter.
- Diff gutter when the file is in the PR: a green bar for added lines, a
  yellow bar for modified, and deleted lines rendered as read-only ghost
  lines with a red tint and no line number. Ghost lines are phantom text so
  offsets into the real file stay correct for the LSP.
- Selection and copy. Cmd-A, Cmd-C, drag select, double-click word,
  triple-click line.
- Cmd-F: find in file, with match count, next and previous. Highlights all
  matches.
- Cmd-click or F12 on a symbol: go-to-definition. If the target is in the
  worktree, open it in a tab at that line. If the target is outside (a
  dependency), open it read-only in a tab marked "external".
- Hover after 400ms still: a popup with the LSP hover contents, rendered as
  plain monospace text. Markdown rendering is milestone 3.
- Diagnostics: squiggles under error and warning ranges, with the message
  on hover.
- A "next change" and "previous change" pair of keys (F7 and Shift-F7)
  that jump between hunks.
- The editor never receives a keystroke that would mutate the buffer.
  Enforce this in one place, not per key.

### Status line

Left: rust-analyzer state. `starting`, `indexing`, `indexing 42%` when the
server reports progress, `ready`, or `failed: <reason>`. Right: cursor line
and column of the active editor.

## LSP client

In `perch-lsp`, a new crate with no UI dependencies.

- `lsp-types` for the protocol, `async-lsp` for transport. If `async-lsp`
  fights the client role, replace it with `lsp-server` plus a hand-written
  request map and note why.
- One server process per worktree, spawned with the worktree as root. Killed
  when the worktree is closed or the app exits.
- Requests this milestone: `initialize`, `initialized`, `textDocument/
  didOpen`, `textDocument/didClose`, `textDocument/hover`,
  `textDocument/definition`, `shutdown`, `exit`. Notifications handled:
  `textDocument/publishDiagnostics`, `$/progress` for the indexing
  percentage, `window/logMessage` to the log.
- Open documents are sent with `didOpen` once and never `didChange`, since
  nothing changes.
- Locate `rust-analyzer` on PATH, then `~/.cargo/bin`, then
  `rustup which rust-analyzer`. If none, the status line says so and the
  editor works without LSP.
- Requests time out at five seconds. A timeout is not an error, just a
  missing hover.

The client is generic. rust-analyzer is the only server configured, but the
config is a table of (file extension, server binary, args), so gopls is a
one-line addition later.

## Tree-sitter

In `perch-syntax`, another UI-free crate.

- Highlighting via `tree-sitter-highlight` with the standard capture names
  mapped to theme colours in `theme.rs`.
- Parse on open, on a background thread, and hand the highlight spans to the
  editor. Files under a megabyte are parsed synchronously; larger ones show
  plain text first and highlight when ready.
- Language chosen by extension. Unknown extensions are plain text.

## Diff

In `perch-git`, alongside the clone and worktree code.

- `imara-diff` over base blob and head blob, line-based, producing hunks of
  `Equal`, `Insert`, `Delete` runs.
- A `LineStatus` per head line: `Unchanged`, `Added`, `Modified`. A line is
  modified when it sits in a hunk that has both deletes and inserts.
- Deleted runs are attached to the head line that follows them, for the
  ghost-line rendering.
- Renames detected by `gix`'s rename tracking, similarity at 50%.

## Crates after this milestone

```
crates/
  perch-github/    m1, plus the pulls endpoint and PR cache
  perch-git/       clone, fetch, worktree, diff, tree listing
  perch-lsp/       LSP client
  perch-syntax/    tree-sitter highlighting
  perch-app/       m1 UI plus the review view
```

Dependency direction: `perch-app` depends on the other four. None of the
other four depend on each other, and none depend on floem.

## Inbox changes

- PR rows become clickable. Hover shows a pointer. Issue rows stay inert.
- A PR that is being prepared shows a small spinner in its row.
- The sidebar's Inbox row returns to the list from the review view.
- Cmd-[ goes back to the inbox from the review view.

## Threading

Unchanged model. The backend runtime gains:

- A `Prepare { owner, repo, number }` command that runs steps 2 through 6 and
  emits `PrepareProgress` events, then `Ready { worktree, files, pr }`.
- An `OpenFile { path }` command that reads the blob, parses it, computes
  the diff if applicable, and emits `FileLoaded`.
- LSP requests as commands with a request id, answered by events carrying
  the same id. The UI keeps a map of pending ids and drops stale answers
  when the user has moved on.

## Build order

1. `perch-git`: bare clone, fetch a PR ref, worktree add, diff base to head,
   list tree. A CLI example that takes `owner/repo#N` and prints the changed
   files with counts. Tests against a temp repo created in the test with
   `git init`. Commit.
2. `perch-syntax`: highlight a Rust file, return spans. Test on a fixture.
   Commit.
3. Review view with the rail in Changed mode and a plain read-only editor.
   No highlighting, no diff, no LSP. Click a PR, see its files, open one.
   Commit.
4. Tree-sitter highlighting in the editor. Commit.
5. Diff gutter and ghost lines. Next and previous change. Commit.
6. All files mode with the tree. Commit.
7. Tabs. Commit.
8. `perch-lsp`: spawn rust-analyzer, initialize, hover. CLI example that
   prints hover for a file and position. Commit.
9. Hover popup in the editor. Status line with indexing progress. Commit.
10. Go-to-definition, including external files. Diagnostics squiggles.
    Commit.
11. Find in file. Commit.
12. Worktree sweep at startup. Progress card with cancel. Commit.

Steps 1, 2, and 8 are independent and can happen in parallel.

## Acceptance

- From a fresh data directory, click slatedb PR 2091: clone completes with
  visible progress, the changed files appear, opening `src/compactor.rs`
  shows highlighted Rust with the diff gutter, within thirty seconds on a
  normal connection.
- Cmd-click on a type from slatedb's own code opens its definition in a new
  tab at the right line.
- Hover on a `Result` shows rust-analyzer's type text.
- Switch to All files, open a file the PR did not touch: plain highlighted
  Rust, no gutter marks.
- Open a second PR from the same repo: no clone, one fetch, under five
  seconds to the file list.
- Kill rust-analyzer while a PR is open: status line shows failed, editor
  still works, hover silently does nothing.
- Memory with one PR open and rust-analyzer indexed on slatedb is whatever
  rust-analyzer needs plus under 150 MB for Perch itself.
- `cargo test` green, `cargo clippy` clean, on all five crates.

## Things the implementer should not do

- Do not add a settings screen. rust-analyzer's path is discovered, not
  configured.
- Do not render markdown in hover popups. Plain text.
- Do not implement split diff, even if the editor makes it look easy.
- Do not embed the token in any git URL or config file.
- Do not use `git2` for network operations.
- Do not block the UI thread on clone, fetch, parse, or LSP. If a step can
  take more than 50ms, it belongs in the backend.
- Do not make the inbox depend on the review crates. `perch-app` may import
  them; `perch-github` may not.
