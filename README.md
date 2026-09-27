# Perch

Perch is a native macOS app for reviewing pull requests and doing work with
coding agents. It puts your GitHub notifications, a code browser with
rust-analyzer, and Claude Code and Codex in one window, with a git worktree
behind every item.

- **Inbox.** Your GitHub notifications, in GitHub's order. Pull requests and
  issues open inside Perch; everything else opens on github.com. Mark read,
  mark unread, Done and Unsubscribe work as they do on GitHub.
- **Conversation.** An item's description, comments, reviews and timeline in
  one column. React, reply, resolve threads, merge, or close a pull request
  as an issue.
- **Code.** The item's worktree with its diff, a file tree, `Cmd-P` Go to
  File, find, and go-to-definition, references and call hierarchy through
  rust-analyzer. Review threads show up on the lines they're about.
- **Agents.** Ask Claude Code or Codex about an item or a selection of lines.
  On your own branches, switch to Work and let the agent edit the worktree
  in a sandbox.
- **Reviews.** Run Claude Code's `/code-review` and Codex's review over a
  branch. Findings on an open pull request land as your drafts, and nothing
  posts to GitHub until you submit the review.
- **New work.** `Cmd-N` cuts a branch and worktree from any repository you've
  configured and hands your first message to an agent. When it's ready, open
  the pull request from Perch.

## Requirements

- macOS
- A GitHub token (the easiest way is `gh auth login`)
- Optional: [Claude Code](https://claude.com/claude-code) 2.1.223+ and/or
  [Codex CLI](https://github.com/openai/codex) 0.155.1+ for agents and reviews
- Optional: `rust-analyzer` on your `PATH` for code navigation

## Install

Perch is built from source. The toolchain is pinned in `rust-toolchain.toml`,
so `rustup` picks up the right compiler.

```bash
./scripts/bundle.sh
```

This builds a release binary and wraps it in `Perch.app` under the cargo
target directory. Copy it to `/Applications`, or run it directly:

```bash
cargo run --release -p perch-app
```

## Setup

### GitHub

Perch looks for a token in this order: `GITHUB_TOKEN`, `GH_TOKEN`,
`gh auth token`, then `token` in `~/.perch/config.toml`. Apps launched from
Finder don't see your shell's environment, so `gh auth login` is the simplest
route. If you sign in after Perch starts, it picks the token up on its next
poll.

The token needs to read and update notifications and read the repositories
you open. Writing back needs more:

| To | Permission |
|---|---|
| Comment, review, resolve threads, react on a PR, open a PR | Pull requests: write |
| Merge, push a branch | Contents: write |
| React on an issue, Close as issue | Issues: write |

### Repositories

The inbox works with no config at all. Add repositories to
`~/.perch/config.toml` when you want Perch to find your local branches and
worktrees, or to start new work there:

```toml
[[repo]]
github = "slatedb/slatedb"
roots = ["~/Code/slatedb"]     # your local clone(s)

[[repo]]
roots = ["~/Code/side-project"] # a local-only repo works too
```

You can also add one from the new-work view with **Add repository…**.

Perch never crawls a repository. It fetches a pull request or issue when you
open it.

### Agents

Perch finds the `claude` and `codex` CLIs on its own. To change the defaults:

```toml
[agent]
model = "sonnet"
effort = "medium"
max_concurrent_runs = 4

[agent.codex]
models = ["gpt-6-astra"]

[work]
branch_prefix = "chris/"
approvals = "auto"             # or "ask"
```

## Keyboard shortcuts

| Action | Key |
|---|---|
| New work | `Cmd-N` |
| New window | `Cmd-Shift-N` |
| Conversation, Code | `Cmd-Shift-1`, `Cmd-Shift-2` |
| Go to File | `Cmd-P` |
| Find in file | `Cmd-F` |
| File tree | `Cmd-Shift-E` |
| Agents | `Cmd-Shift-A` |
| Next, previous tab | `Cmd-}`, `Cmd-{` |
| Close tab, then window | `Cmd-W` |
| Go to Definition | `F12`, `Cmd-click` |
| Find All References | `Shift-F12` |
| Go to Implementations | `Cmd-F12` |
| Call Hierarchy | `Shift-Alt-H` |
| Back, Forward | `Ctrl--`, `Ctrl-Shift--` |

## Agent safety

Agents read pull requests, comments and code that anyone could have written,
so Perch assumes any of it might try to steer them.

- **Ask** runs are read-only. Claude Code's shell is sandboxed with no network
  and no writes; Codex runs in its `read-only` sandbox.
- **Work** runs can edit their worktree (and any paths you list under
  `[repo.work]`), and can't push or reach GitHub. Anything outside the sandbox (a network host, a command, a
  commit) goes to the CLI's auto-reviewer or to you as an approval card with
  Allow once, Always, and Deny.
- A repository's own `.claude/`, `.mcp.json` and `.codex/` config is ignored
  unless you set `trust = true` for it.
- Agents don't get your GitHub token or other `*_TOKEN`/`*_SECRET`-style
  variables.
- Nothing an agent writes reaches GitHub unless you post it.

The CLIs still read `CLAUDE.md` and `AGENTS.md`, and an agent can read any
file your user can.

## Data

Everything lives under `~/.perch` (or `$PERCH_HOME`): the config, a SQLite
database, worktrees, clones, and agent state. The database holds your local
conversations, drafts and review history, so don't treat it as a cache.

## More

[docs/reference.md](docs/reference.md) describes every feature in detail:
how the inbox syncs, what agents are told, sandbox rules, review scopes,
approvals and grants, and the rest.

## Development

```bash
cargo test --workspace
```
