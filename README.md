# Perch

Perch is a native macOS coding and review tool. It combines agent-driven
worktrees, code browsing and LSP support, GitHub reviews and issues, and local
human/agent conversations in a compact Rust application built with Floem and
Kenn Forge.

## GitHub inbox

Perch builds its inbox from GitHub's REST notifications list. It preserves the
API's order and unread state, and caches the latest response locally so the
sidebar can paint immediately. Perch requests `all=true` so read threads remain
visible alongside unread ones.

Polling respects GitHub's `X-Poll-Interval` and paginates the full response.
Perch performs an unconditional reconciliation at startup, on manual refresh,
and on the first scheduled poll after 15 minutes; polls between reconciliations
use `Last-Modified`. The full fetch repairs the sidebar when GitHub's validator
does not change with the notification list.

- Opening a notification marks its GitHub thread read. Read threads remain in
  the inbox.
- **Mark Unread** restores the unread reminder in Perch for the current update.
  Opening it clears the reminder; later GitHub state takes precedence. GitHub's
  public API has no mark-unread operation, so this does not change github.com.
- **Done** marks the thread done on GitHub and remembers that exact update in
  Perch, so GitHub's `all=true` history cannot put it back. Newer activity
  clears the local mark and lets the thread return.
- **Unsubscribe** ignores future activity on the thread, marks its current
  notification done, and remembers it the same way.
- Pull requests and issues open inside Perch. Other notification types remain
  first-class inbox rows and open on GitHub.

GitHub's public REST notification payload does not expose whether a thread is
currently in the web Inbox or Done, and `all=true` can include Done history.
Perch can therefore keep its own Done actions stable, but cannot reliably
mirror Done or Restore actions performed on github.com.

These actions affect only the notification or its local reminder. They do not
archive or delete a Perch item, worktree, or local conversation.

Perch does not crawl a repository's open items or history. A pull request or
issue is fetched when a notification is opened or when the user opens it by
repository and number. Its current data and discussion may be cached locally;
closed history is never backfilled.

## Reactions

Each GitHub entry on Conversation shows the reactions on it under its text:
the description, every comment, and every review summary and line comment. A
pill is one reaction and its count, blue when yours is among them. Clicking a
pill adds or removes yours, and the face at the right of an entry's byline
opens a picker with all eight. A reaction is shown as soon as it is clicked
and put back if GitHub refuses it.

Reactions are read with the discussion through GitHub's GraphQL API, a
hundred entries to a request, and left the same way. An entry cached by an
older build of Perch has no reactions until it is fetched again.

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
its repository. Commenting on, approving, or requesting changes on a pull
request requires `Pull requests: write` permission, as does reacting to a pull
request or anything said on it; reacting on an issue requires `Issues: write`.

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
  `CODEX_API_KEY`) are the exception, since without them there is no run.
  Credentials for other providers, such as `AWS_SECRET_ACCESS_KEY`, fall
  under the rule, so a CLI configured that way must be signed in instead.
- **Cleaned quotes.** Text quoted into a prompt keeps the words a reader
  sees and loses the rest: HTML tags and the comments GitHub renders as
  nothing, link titles and image alt text, and invisible formatting
  characters, however they were written. It is set between
  `[BEGIN untrusted text: …]` and `[END untrusted text]` lines, a line
  inside it that imitates a marker is spoiled, and your question comes
  last. The review history export under `knowledge/reviews/` is cleaned
  the same way.
- **Inert links.** A link in an answer or a comment does nothing until it
  is clicked. Hovering it shows the full destination; a click opens `http`
  and `https` links only.

What remains: the CLIs still read the repository's `CLAUDE.md` and
`AGENTS.md` as instructions, along with your own user-level configuration,
and the agent can read any file your user can, not only the worktree. What
it reads can reach you only through its answer.

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

## Code navigation

Files reads like an IDE when rust-analyzer is installed. Rest the pointer on
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

One location opens; several open the panel and the reader picks. The call
hierarchy is a tree: a chevron on a row asks for that row's callers, and a
toggle in the panel's header turns it to callees. The caret resting on an
identifier tints its other uses in the file. All of it is read-only, and all
of it is rust-analyzer: while the server is absent or failed, the status line
says so and nothing else happens.

## Development

```sh
cargo test --workspace
cargo run -p perch-app
```
