# Perch

I got tired of flipping between github.com, VS Code, and Claude/Codex, so I
made Perch to combine them into one native application.

Features:

* Uses `gh`, `claude`, and `codex` CLIs for everything.
* Fully integrated with GitHub's inbox, PRs, and issues.
* Completely browsable codebase for each PR and code review.
* Language server protocol (LSP) support, so you can "find references",
  "open call hierarchy", and so on.
* Threaded GitHub-style conversations with humans and agents.
* Code walkthroughs for PRs.
* Fully integrated client-side Claude/Codex reviews.

## Install

You need:

- An Apple Silicon or Intel Mac, with git
- The [GitHub CLI](https://cli.github.com/), signed in with `gh auth login`
- For agents: [Claude Code](https://claude.com/claude-code) 2.1.223+ and/or
  [Codex CLI](https://github.com/openai/codex) 0.155.1+, signed in

Then install the latest release:

```bash
gh release download --repo perch-code/perch --pattern Perch-macos-arm64.tar.gz -O - | tar -xz -C /Applications
```

On an Intel Mac, download `Perch-macos-x86_64.tar.gz` instead. Perch isn't
notarized yet, and downloading with `gh` rather than a browser keeps macOS
from quarantining it. After that, Perch checks for new releases. Click
"Restart to update" to upgrade when one is detected.

## Setup

Configure Perch with `~/.perch/config.toml`. See
[docs/configuration.md](docs/configuration.md) for all settings.

### GitHub

Perch looks for a GitHub token in this order: `GITHUB_TOKEN`, `GH_TOKEN`,
`gh auth token`, then `token` in the config. Apps launched from Finder don't
see your shell's environment, so `gh auth login` is the simplest route.
### Repositories

```toml
[[repo]]
github = "slatedb/slatedb"
roots = ["~/Code/slatedb"]      # your local clone(s)

[[repo]]
roots = ["~/Code/side-project"] # a local-only repo works too
```

`roots` is where Perch finds your branches and worktrees. Removing a
repository leaves its checkouts and worktrees on disk.

## Language servers

Perch starts language servers when you browse a codebase. The following LSPs
are currently supported:

| Language | Server | Perch installs it with |
|---|---|---|
| Rust | rust-analyzer | rustup, into the checkout's toolchain |
| Go | gopls | `go install` (needs Go) |
| TypeScript, JavaScript | typescript-language-server | `npm install` (needs Node) |
| Java | jdtls | a checksummed download from Eclipse (needs JDK 21+) |
| Python | ty | a checksummed download from GitHub |

Servers other than rust-analyzer go in `~/.perch/servers`. Perch never
installs Go, Node or a JDK, and never runs `npm install` in a checkout, so a
TypeScript checkout without `node_modules` can't resolve its packages' types.

rust-analyzer and jdtls run a checkout's own code as they load it: build
scripts and proc macros, or a Gradle or Maven build. Perch starts them on its
own only for your own work in a configured repository. For anything else,
such as someone else's pull request, it asks first with **Trust and start**.
You can trust a server for one item, which covers commits pushed to it
later, or for a whole repository. To trust
every server on everything in a repository:

```toml
[[repo]]
github = "slatedb/slatedb"
lsp.trust = true
```

## Agent safety

Agents read pull requests, comments and code that anyone could have written,
so Perch assumes any of it might try to steer them.

- **Ask** runs are read-only. Claude Code's shell is sandboxed with no
  network and no writes; Codex runs in its `read-only` sandbox.
- **Work** runs can edit their worktree and any paths listed under
  `[repo.work] write`, and reach only the hosts listed under
  `[repo.work] network`. Anything else, such as another host, a command or
  a commit, goes to the CLI's auto-reviewer or to you as an approval card.
  What gets approved runs outside the sandbox.
- `approvals = "never"` under `[repo.work]` approves every request without
  asking. The composer's approvals picker changes this for one item until
  Perch quits.
- A repository's own `.claude/`, `.mcp.json` and `.codex/` config is ignored
  unless `[repo.work] trust = true`.
- GitHub tokens and variables named like secrets (`*_TOKEN`, `*_KEY`,
  `*_SECRET` and so on) are removed from an agent's environment, except the
  keys Claude Code and Codex sign in with.
- Perch posts nothing an agent writes to GitHub until you post it.

Ask and Work runs still read `CLAUDE.md` and `AGENTS.md`; reviews don't. An
agent can read any file your user can.

## Feedback

**Send Feedback…** in the Perch menu, or the feedback button at the bottom
of the sidebar, files a bug report or an idea as an issue on
`perch-code/perch`. Error and warning toasts have a **Report** button that
starts one.

A report includes Perch's version, your OS, the agent CLIs and language
servers it found, your config without the token, recent errors and the end
of the log. Perch redacts tokens and secret-looking values, most email addresses, your
home directory and user name in paths, and the other repositories it knows
about. You see
the whole issue, and can edit it, before anything is sent. For a bug, Claude
or Codex can look into it first, read-only, and add notes.

If your token can't file issues, **Open on GitHub** copies the report and
opens GitHub's new-issue page.

## Data

Perch keeps its data under `~/.perch` (or `$PERCH_HOME`): the config, a
SQLite database, clones, worktrees, agent state, installed language servers,
and a log file a day, the newest seven kept. The database holds your local
conversations, drafts and review history, so it isn't a cache.

Perch removes the worktrees it made once they're done with: nothing modified
or untracked, no commit that isn't somewhere else, and the pull request
merged or closed, or the item not opened in 14 days. The branch stays, and
**Restore Worktree** brings the directory back. Like `git worktree remove`,
it treats ignored files such as `.env` as disposable. Worktrees under your
`roots` are yours, and Perch never removes them.

## Building from source

You need rustup (the repository pins the toolchain), Python 3 and Xcode's
command-line tools.

```bash
./scripts/bundle.sh
```

This builds `Perch.app` under the cargo target directory. Copy it to
`/Applications`, or run Perch directly:

```bash
cargo run --release -p perch-app
```

If you change Perch and run it often, use the `local` profile. It skips
link-time optimization and builds incrementally, for a slightly slower app.

```bash
cargo run --profile local -p perch-app
```

## Releasing

Bump `version` under `[workspace.package]` in `Cargo.toml`, run `cargo check`
to update `Cargo.lock`, commit both, then push a matching tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

The release workflow builds `Perch.app` for Apple Silicon and Intel and
publishes both tarballs to a GitHub release. A tag with a hyphen, like
`v0.2.0-beta.1`, becomes a prerelease, which neither the updater nor
`gh release download` picks up.
