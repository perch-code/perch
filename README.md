# Perch

Perch is a native macOS coding and review tool. It combines agent-driven
worktrees, code browsing and LSP support, GitHub reviews and issues, and local
human/agent conversations in a compact Rust application built with Floem and
Kenn Forge.

## GitHub inbox

GitHub notifications are Perch's inbox. Perch reads the complete current
inbox from GitHub's REST API, keeps GitHub's ordering and unread state, and
caches the latest response locally so the sidebar can paint immediately.
Polling follows GitHub's `X-Poll-Interval`, uses `Last-Modified` when
available, and paginates the full result.

- Opening a notification marks its GitHub thread read. Read threads remain in
  the inbox.
- **Done** removes the thread from GitHub's inbox.
- **Unsubscribe** ignores future activity on the thread and marks the current
  notification done.
- Pull requests and issues open inside Perch. Other notification types remain
  first-class inbox rows and open on GitHub.

These actions only change the GitHub notification. They do not archive or
delete a Perch item, worktree, or local conversation.

Perch does not crawl a repository's open items or history. A pull request or
issue is fetched when a notification is opened or when the user opens it by
repository and number. Its current data and discussion may be cached locally;
closed history is never backfilled.

## Authentication

Perch calls GitHub's API directly. It resolves credentials in this order:

1. `GITHUB_TOKEN`
2. `GH_TOKEN`
3. `gh auth token`
4. `token` in `~/.perch/config.toml`

Using `gh auth login` is the simplest setup for the desktop app because a GUI
launch does not normally inherit shell environment variables. The credential
must be allowed to read and update notifications and to read the repositories
you open.

## Configuration

Perch reads `~/.perch/config.toml` (or `$PERCH_HOME/config.toml`). A repository
can have a GitHub identity, local roots, or both:

```toml
poll_interval_secs = 60

[[repo]]
github = "slatedb/slatedb"
name = "slatedb"
roots = ["~/Code/slatedb"]
```

`name` is only the sidebar label. `roots` lets Perch discover existing local
branches and worktrees. Neither setting causes a repository-wide GitHub crawl.

Perch's database, avatars, managed clones, worktrees, and agent state live
under `~/.perch`. The database also holds local items, conversations, and
review state, so it is not disposable. GitHub notifications and fetched
remote data within it are refreshed on demand.

Older `~/.perch/knowledge` directories are no longer read. They can be
removed manually after any files worth keeping have been copied elsewhere.

## Development

```sh
cargo test --workspace
cargo run -p perch-app
```
