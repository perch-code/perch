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
- Optional, for code navigation in Rust, Go, TypeScript, JavaScript, Java
  and Python: a language server for each (see [Language servers](#language-servers)).
  Perch offers to install one that's missing.

## Install

Perch runs on Apple Silicon and Intel Macs. With `gh` signed in to an account
that can read this repository, install the latest release with:

```bash
gh release download --repo perch-code/perch --pattern Perch-macos-arm64.tar.gz -O - | tar -xz -C /Applications
```

On an Intel Mac, download `Perch-macos-x86_64.tar.gz` instead.

Downloading through `gh` rather than a browser keeps macOS from quarantining
the app, which isn't notarized.

After that, Perch updates itself. Every six hours it asks GitHub, with the
same token it reads notifications with, whether there's a newer release.
When there is, it downloads and unpacks it under `~/.perch/updates`, and
**Restart to update** takes the login's place in the drawer's footer. A
restart swaps the new app in where the old one was and reopens it. A Perch
run from source (`cargo run`) never updates itself.

To build from source instead:

```bash
./scripts/bundle.sh
```

This builds a release binary and wraps it in `Perch.app` under the cargo
target directory. Copy it to `/Applications`, or run it directly:

```bash
cargo run --release -p perch-app
```

If you change Perch and run it often, use the `local` profile. It skips
link-time optimization, which takes most of the time of a release build, and
it builds incrementally. The app runs a little slower than a release build.

```bash
cargo run --profile local -p perch-app
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
| Send feedback | Issues: write on `perch-code/perch` |

A fine-grained token can only write to repositories its owner picked, so it
can't file feedback on Perch's repository unless it was made for `perch-code`.
When filing is refused, Perch copies the report to your clipboard and opens
GitHub's new-issue page instead.

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
Hover a repository in that menu and click its × to take its `[[repo]]` back out;
its checkouts and Perch's worktrees stay on disk.

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
| TypeScript, JavaScript | typescript-language-server | `PATH`, with the TypeScript installed with it | `npm install` (needs Node) |
| Java | jdtls | `PATH` (Homebrew's, or a distribution's) | a checksummed download from Eclipse (needs a JDK 21+) |
| Python | ty | `PATH` | a checksummed download from GitHub (needs nothing else) |

When a server is missing, a toast says so with an **Install** button; its
tooltip is the command it runs. Perch downloads nothing without that click,
or **Update** on an install of its own that's older than the version it
pins. rust-analyzer goes into the checkout's toolchain, where it matches
the compiler; the others go in `~/.perch/servers`. Perch never installs Go,
Node or a JDK, and never runs `npm install` in a checkout, so a TypeScript
checkout without `node_modules` can't resolve its packages' types.

The download runs on a JDK 21 or later. On macOS that is the one
`/usr/libexec/java_home` picks, else `JAVA_HOME`. Elsewhere it is
`JAVA_HOME`, else the JDK the `java` on `PATH` belongs to, else the newest
in `/usr/lib/jvm`.

rust-analyzer and jdtls run the checkout's own code as they load it:
rust-analyzer its build scripts and proc macros, jdtls its Gradle or Maven
build. Perch starts them by itself only on your own work in a repository in
your config: your branches, the default branch, and your pull requests whose
branch is in that repository or your fork. On anything else, such as someone
else's pull request, a branch you cut from one, or a repository not in your
config, a toast says what would run, with **Trust and start**. Perch
remembers that for the item, commits pushed to it later included; deleting
the item forgets it. Tick **Always trust rust-analyzer in slatedb/slatedb**
under the toast's line before pressing it, and that server starts on
everything in the repository from then on without asking; each server has
its own box. While they wait, the Worktree row on Conversation has a
warning, and its menu has **Trust and Start** and **Always Trust** once the
toast is closed. The same menu, on any item in the repository, has **Stop
Trusting** for a server trusted there. After upgrading, a branch you had
already cut from a pull request, your own included, asks once. gopls,
typescript-language-server and ty start either way. To trust every server on
everything in a repository, pull requests from anyone included:

```toml
[[repo]]
github = "slatedb/slatedb"

[repo.lsp]
trust = true
```

Perch never lets typescript-language-server run a checkout's own TypeScript,
which a pull request could replace; a server on `PATH` older than 2.1, or
without a TypeScript of its own, gives way to Perch's.

### Look

On macOS code is set in SF Mono. Elsewhere Perch takes the first installed
of JetBrains Mono, DejaVu Sans Mono, Noto Sans Mono and Liberation Mono,
or another family you name:

```toml
[ui]
code_font = "Fira Code"
```

Perch is light or dark as the system is. To keep it one or the other, set
`appearance` to `"light"` or `"dark"` (`"system"` is the default):

```toml
[ui]
appearance = "dark"
```

On Linux the system's setting doesn't reach Perch under X11, so there it
is light unless `appearance` says otherwise.

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

The megaphone in the drawer's footer, or **Send Feedback…** in the Perch menu,
files a bug report or an idea as an issue on `perch-code/perch`. An error
or warning toast has **Report**, which starts one about it.

A report carries what Perch knows about itself: its version and commit,
the operating system (on macOS its version and your screens, on Linux the
distribution, kernel and session), the agent CLIs it found, its language servers (which
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
`servers/` to uninstall them. `walkthroughs/` holds the walkthroughs agents
wrote, one file per item and head. Perch's log is in `logs/`, one file a day,
the last seven kept. A downloaded update waits in `updates/` until the
restart that installs it. The database holds your local conversations, drafts
and review history, so don't treat it as a cache.

Perch removes the worktrees it made under `~/.perch` once they're done
with: when nothing in one is modified or untracked, its branch has no
commit that isn't somewhere else, and its pull request has merged or
closed or you haven't opened it in 14 days. The branch stays, and Restore
Worktree brings the directory back. Like `git worktree remove`, it counts
files git ignores, such as `.env`, as disposable. Worktrees under your
`roots` are yours, and Perch never removes them.

## Releasing

Bump `version` under `[workspace.package]` in `Cargo.toml`, commit, then push
a matching tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

The release workflow builds `Perch.app` on an Apple Silicon runner, once
natively and once cross-compiled for Intel, and publishes
`Perch-macos-arm64.tar.gz` and `Perch-macos-x86_64.tar.gz` to a GitHub
release. A running Perch finds the one for its processor within six hours.
A release without it is skipped. A tag with a hyphen, like `v0.2.0-beta.1`, becomes
a prerelease, which neither the updater nor `gh release download` picks up.
