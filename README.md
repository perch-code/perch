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
- **Mark Unread** restores the unread reminder in Perch for the current update.
  Opening it clears the reminder; later GitHub state takes precedence. GitHub's
  public API has no mark-unread operation, so this does not change github.com.
- **Done** removes the thread from GitHub's inbox.
- **Unsubscribe** ignores future activity on the thread and marks the current
  notification done.
- Pull requests and issues open inside Perch. Other notification types remain
  first-class inbox rows and open on GitHub.

These actions affect only the notification or its local reminder. They do not
archive or delete a Perch item, worktree, or local conversation.

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

Schema upgrades use preserving, transactional migrations: local discussions,
agent attempts, session IDs, and activity history survive. Perch refuses an
unsupported or newer schema without deleting it, and a failed migration leaves
the original database in place.

## Agents

Questions about an item or selected lines in its Files view run through Claude
Code or Codex in that item's worktree. Follow-ups remain attached to the local
conversation, and the agents retain their read-only permissions.

The persistent **Agents** control in the status bar opens an app-wide activity
inspector. Active work is grouped by worktree and continues while another item
is open or while the panel is closed. Each row shows the agent, model, effort,
queue or execution time, and latest tool activity. Selecting one shows its
retained request, ordered activity, streamed response, usage, and errors.
Active rows can be stopped, queued rows can be cancelled, and failed, stopped,
or interrupted attempts can be retried without replacing their history.

Perch executes at most four top-level agents by default and queues the rest in
FIFO order. Stopping a run terminates and reaps its process group. Closing
Perch does the same for every process it owns; after an abnormal exit,
nonterminal rows return as Interrupted and are not replayed automatically.

Perch discovers both CLIs automatically. Their optional settings are:

```toml
[agent]
binary = "claude"
model = "sonnet"
effort = "medium"              # low | medium | high | xhigh | max | ultra
models = ["fable", "opus", "sonnet", "haiku"]
max_concurrent_runs = 4

[agent.codex]
binary = "codex"
models = ["gpt-6-astra"]       # omit to use Codex's own model catalog
```

`perch --agents [limit]` prints active runs and retained history without
launching an agent. `perch --ask owner/repo#N path:start-end "question"`
runs the same question flow from a terminal; omit the path to ask about the
whole item.

## Worktree reviews

**Review** runs Claude Code and/or Codex's native review workflow against the
full committed branch diff from the selected base's merge base to `HEAD`. It
works for Perch-managed and registered external worktrees, with or without a
pull request. A dirty worktree is reviewable, but staged and unstaged edits are
not part of the snapshot; Perch labels this `Committed changes only; local
edits are excluded`. Staged-only, individual-commit, and incremental-only
review scopes are not supported. If the committed diff is empty, Perch records
that result without launching an agent.

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
the branch's current committed state.

Review results and follow-ups stay local to Perch. Reviews do not post,
resolve, or comment on GitHub, modify code, run automatically after a push, or
use a hosted review service. Perch also exports each item's review history as
deterministic Markdown at
`$PERCH_HOME/knowledge/reviews/<owner>/<repo>/<item-id>.md` (under
`~/.perch` when `PERCH_HOME` is unset). Re-exporting replaces that stable
document rather than duplicating unchanged entries; unrelated legacy files in
the knowledge directory are not imported.

## Development

```sh
cargo test --workspace
cargo run -p perch-app
```
