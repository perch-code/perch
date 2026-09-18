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
| `Cmd-[` | Back to the inbox from a pull request. |
| `Cmd-W` | Close the open file. |

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

Cache and avatars live beside it, and so do your Saved and Done marks.
Deleting that directory costs a cold start and every mark you have made;
nothing else in Perch is worth keeping, and marks are the one thing GitHub
cannot give back.

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
