# Perch

A native, read-only GitHub notifications inbox for macOS, written entirely in
Rust with [floem](https://github.com/lapce/floem).

Perch is the first milestone of a larger code review tool. Today it does one
thing: show your GitHub notifications the way the web inbox does, with the
same filters, sorting, grouping, and search, and nothing that mutates state.

## What it does

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

## What it deliberately does not do

Nothing in Perch writes to GitHub. There is no mark as read, no mark as done,
no save, no unsubscribe. Rows are not clickable and there is no detail pane.
The Saved and Done views appear in the sidebar to match the web UI, and are
inert: the REST API does not expose them.

## Requirements

- macOS
- Rust 1.97 (pinned in `rust-toolchain.toml`)
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
| `perch-github` | Model, REST client, SQLite cache. No UI dependencies. |
| `perch-app` | Floem UI and the background worker. Binary target `perch`. |

The split is deliberate: data and IO stay testable without a window, and stay
reusable when the review features land.

## Development

```bash
cargo test
cargo clippy --all-targets
```

`cargo run -p perch-github --example dump` prints the inbox to the terminal,
which is the quickest way to check the client and your token independently of
the UI.

`scripts/screenshot.sh out.png` captures the running window. It needs the
screen unlocked; macOS will not composite an app window behind the login
window.
