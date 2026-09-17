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

**The review view**

- Click a pull request and Perch clones the repository, checks the head out
  into a worktree, and shows what changed. A second pull request from the
  same repository costs a fetch rather than a clone.
- A rail of changed files, or the whole worktree, with change markers on
  files the pull request touched.
- A read-only editor with tree-sitter highlighting for Rust, TOML, Markdown,
  YAML, and JSON.
- The unified diff in place: added and modified lines tinted, removed lines
  shown above the code that replaced them, and a gutter carrying real file
  line numbers with change bars.
- Click a symbol and rust-analyzer says what it is, with go-to-definition
  from the same panel.

## What it deliberately does not do

Nothing in Perch writes to GitHub. There is no mark as read, no mark as done,
no save, no unsubscribe. Rows are not clickable and there is no detail pane.
The Saved and Done views appear in the sidebar to match the web UI, and are
inert: the REST API does not expose them.

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

## Configuration

Optional, at
`~/Library/Application Support/com.perch.perch/config.toml`:

```toml
token = "ghp_..."        # only when env vars and gh are unavailable
poll_interval_secs = 60  # a floor; X-Poll-Interval may raise it
```

Cache and avatars live beside it. Deleting that directory is safe and only
costs a cold start.

## Layout

| Crate | Contents |
|---|---|
| `perch-github` | Model, REST client, SQLite cache. |
| `perch-git` | Clone, fetch, worktree, diff, tree listing. |
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
