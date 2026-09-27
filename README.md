# Perch

Perch is a native macOS app for reviewing pull requests and doing work with
coding agents. It puts your GitHub notifications, a code browser with
rust-analyzer, and Claude Code and Codex in one window, with a git worktree
behind every item.

## Requirements

- macOS
- A GitHub token (the easiest way is `gh auth login`)
- Optional: [Claude Code](https://claude.com/claude-code) 2.1.223+ and/or
  [Codex CLI](https://github.com/openai/codex) 0.155.1+ for agents and reviews
- Optional: `rust-analyzer` on your `PATH` for code navigation

## Install

Build Perch from source.

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

Perch looks for a GitHub token in this order: `GITHUB_TOKEN`, `GH_TOKEN`,
`gh auth token`, then `token` in `~/.perch/config.toml`. Apps launched from
Finder don't see your shell's environment, so `gh auth login` is the simplest
route.

The token needs to read and update notifications and read the repositories
you open. Writes also need:

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

You can also add one from the new-work (the `+`) view with **Add repository…**.

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

## Agent safety

Agents read pull requests, comments and code that anyone could have written,
so Perch assumes any of it might try to steer them.

- **Ask** runs are read-only. Claude Code's shell is sandboxed with no network
  and no writes; Codex runs in its `read-only` sandbox.
- **Work** runs can edit their worktree (and any paths you list under
  `[repo.work]`), and can't push or reach GitHub. Anything outside the sandbox (a network host, a command, a
  commit) goes to the CLI's auto-reviewer or to you as an approval card with
  Allow once, Always, and Deny. `approvals = "never"` under a repository's
  `[repo.work]` allows all activity without ever asking for approval.
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

Perch removes the worktrees it made under `~/.perch` once they're done
with: when nothing in one is modified or untracked, its branch has no
commit that isn't somewhere else, and its pull request has merged or
closed or you haven't opened it in 14 days. The branch stays, and Restore
Worktree brings the directory back. Like `git worktree remove`, it counts
files git ignores, such as `.env`, as disposable. Worktrees under your
`roots` are yours, and Perch never removes them.
