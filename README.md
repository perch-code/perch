# Perch

Perch is a native macOS app for reviewing pull requests and doing work with
coding agents. It puts your GitHub notifications, a code browser with
language servers, and Claude Code and Codex in one window, with a git
worktree behind every item.

## Requirements

- macOS
- A GitHub token (the easiest way is `gh auth login`)
- Optional: [Claude Code](https://claude.com/claude-code) 2.1.223+ and/or
  [Codex CLI](https://github.com/openai/codex) 0.155.1+ for agents and reviews
- Optional, for code navigation in Rust, Go, TypeScript, JavaScript and
  Java: a language server for each (see [Language servers](#language-servers)).
  Perch offers to install one that's missing.

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
| Send feedback | Issues: write on `criccomini/perch` |

A fine-grained token can only write to repositories its owner picked, so it
can't file feedback on Perch's repository unless it's yours. When filing is
refused, Perch copies the report to your clipboard and opens GitHub's
new-issue page instead.

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
model = "opus"
effort = "high"
max_concurrent_runs = 4

[agent.codex]
models = ["gpt-6-astra"]

[work]
branch_prefix = "chris/"
approvals = "auto"             # or "ask"
```

### Language servers

Perch starts the server for each language a pull request changes, and
others as you open their files:

| Language | Server | Perch looks for it | Perch can install it with |
|---|---|---|---|
| Rust | rust-analyzer | the checkout's rustup toolchain | `rustup component add` (needs rustup) |
| Go | gopls | `PATH`, `~/go/bin` | `go install` (needs Go) |
| TypeScript, JavaScript | typescript-language-server | `PATH` | `npm install` (needs Node) |
| Java | jdtls | `PATH` (Homebrew's) | a checksummed download from Eclipse (needs a JDK 21+) |

When a server is missing, a toast says so with an **Install** button; its
tooltip is the command it runs. Perch downloads nothing without that click,
or **Update** on an install of its own that's older than the version it
pins. rust-analyzer goes into the checkout's toolchain, where it matches
the compiler; the others go in `~/.perch/servers`. Perch never installs Go,
Node or a JDK, and never runs `npm install` in a checkout, so a TypeScript
checkout without `node_modules` can't resolve its packages' types.

Language servers run project code: rust-analyzer runs build scripts and
proc macros, and jdtls runs a Gradle build when it imports one. Keep that
in mind on a pull request from someone you don't know.

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
- The agent that looks into a bug report is an Ask run in the open checkout
  (or `~/.perch`), and its notes go into the report's preview, not to
  GitHub, until you file it.

The CLIs still read `CLAUDE.md` and `AGENTS.md`, and an agent can read any
file your user can.

## Feedback

The bug icon at the top right, or **Send Feedback…** in the Perch menu,
files a bug report or an idea as an issue on `criccomini/perch`. An error
or warning toast has **Report**, which starts one about it.

A report carries what Perch knows about itself: its version and commit,
macOS and your screens, the agent CLIs it found, its language servers (which
binary, from where, how much memory), where the token came from, your config
with the token and repository names left out, the session's recent errors,
and the end of its log. For a bug, Claude or Codex can look into it first,
read-only and offline, and add notes. Tokens and secret-looking values,
emails, your home directory and user name, and every repository but Perch's
are redacted. You see the whole issue, and can edit it, before anything is
sent. If it's already been reported, you can comment on that issue instead,
or +1 it, which also subscribes you.

## Data

Everything lives under `~/.perch` (or `$PERCH_HOME`): the config, a SQLite
database, worktrees, clones, agent state, and in `servers/` the language
servers Perch installed and jdtls's data for each checkout. Delete
`servers/` to uninstall them. Perch's log is in `logs/`, one file a day,
the last seven kept. The database holds your local
conversations, drafts and review history, so don't treat it as a cache.

Perch removes the worktrees it made under `~/.perch` once they're done
with: when nothing in one is modified or untracked, its branch has no
commit that isn't somewhere else, and its pull request has merged or
closed or you haven't opened it in 14 days. The branch stays, and Restore
Worktree brings the directory back. Like `git worktree remove`, it counts
files git ignores, such as `.env`, as disposable. Worktrees under your
`roots` are yours, and Perch never removes them.
