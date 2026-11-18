# Configuration

Perch reads `~/.perch/config.toml`, or `$PERCH_HOME/config.toml` when
`PERCH_HOME` is set. Every setting is optional.

`PERCH_HOME` is also where the cache, the checkouts and the logs live, and
only one engine may use it at a time: a second Perch, or a `perchd`, on the
same home refuses to start and says which process has it (see
[One engine to a Perch home](reference.md#one-engine-to-a-perch-home)).
`perchd` reads the config.toml of the machine it runs on.

`PERCH_SSH`, in the app's environment, names the ssh program the app runs
to reach a host, send its `perchd` there and open a terminal on it, in
place of `ssh`: a wrapper that adds `-F <config>`, for tests.

`PERCH_IDLE_EXIT_SECS`, in a host's login environment, is how many seconds
the engine the app starts there stays with no client and no agent run
before it stops, in place of 900.

With `[remote] host` set, the app uses the engine on that host (see
[An engine on another machine](reference.md#an-engine-on-another-machine))
and the settings split by whose they are. `[ui]`, `[display]`,
`[keybindings]` and `[remote]` are the app's: they are read from and
written to the Mac's config.toml. Everything else is the engine's, in the
host's config.toml, and Settings writes it there through the engine. The
Mac's file can still hold engine settings from before; they are not read
while a host is set. The host's engine looks again at what the host lacks
each time its config.toml changes, so a `[[repo]]` added there, or a
`token`, clears its check (see
[What the app checks](reference.md#what-the-app-checks)).

Settings (`Cmd-,`, or the gear at the foot of the sidebar) edits this file.
It changes only the keys you touch, so your comments and layout stay. Edits
you make by hand while Perch runs are taken up within a couple of seconds.
Everything applies at once except `[display]` and `[ui] code_font`, which
wait for a restart. Settings has no control for `token`, or for a
repository's `github` and `root`.

| Key | Default | What it does |
|---|---|---|
| `token` | none | GitHub token, used only when the env vars and `gh` have none; looked for again only when Perch has no token or GitHub refuses it |
| `poll_interval_secs` | `60` | Shortest time between inbox polls; GitHub can ask for longer |
| `github_host` | none | A GitHub Enterprise host whose remotes count as GitHub |
| `[agent] binary` | `"claude"` | Claude Code executable, a name on `PATH` or a path |
| `[agent] model` | `"opus"` | Model the composer's picker starts on |
| `[agent] effort` | `"high"` | Effort the picker starts on: `low`, `medium`, `high`, `xhigh`, `max` or `ultra` |
| `[agent] models` | `["fable", "opus", "sonnet", "haiku"]` | Claude models the picker offers |
| `[agent] max_concurrent_runs` | `4` | Agents running at once (`1` for Gemini); the rest queue |
| `[agent.codex] binary` | `"codex"` | Codex executable, a name on `PATH` or a path |
| `[agent.codex] models` | Codex's own list | Codex models the picker offers |
| `[agent.gemini] binary` | `"agy"` | Antigravity CLI executable, a name on `PATH` or a path |
| `[agent.gemini] models` | `agy`'s own list | Gemini models the picker offers |
| `[agent.opencode] binary` | `"opencode"` | OpenCode 2.3.0+ executable, a name on `PATH` or a path |
| `[agent.opencode] models` | OpenCode's own list | Provider-qualified IDs from `opencode models` |
| `[work] branch_prefix` | `""` | Put before branch names Perch fills in, such as `"chris/"` |
| `[work] approvals` | `"auto"` | Who answers a Work run's requests outside its sandbox: `"auto"` (the CLI's auto-reviewer, then you) or `"ask"` (you) |
| `[work] approval_timeout_mins` | `15` in the app, `480` under `perchd` | Minutes a request to you waits before it is denied |
| `[keybindings]` *action* | the action's keys | Keys for an action by its id: a string such as `"super+t"`, a list of them, or `""` for none; see [Keyboard shortcuts](reference.md#keyboard-shortcuts) |
| `[ui] appearance` | `"system"` | `"light"`, `"dark"` or `"system"` |
| `[ui] code_font` | none | Code font off macOS; on macOS code is always SF Mono; applies after a restart |
| `[display] zoom` | `1.0` | Scales the whole window, `0.75` to `2.0`; applies after a restart |
| `[display] code_font_size` | `12` | Code font size before zoom, `8` to `32`; applies after a restart |
| `clone_root` | `~` | The folder a repository added by its name is cloned into, such as `"~/Code"`; read by an engine on a host, which is where repositories are added by name |
| `[remote] host` | none | SSH host to use when there is no saved window session (for example `"chris@devbox:2222"`). Perch subsequently reopens each window in its last environment. |
| `[remote] command` | none | What ssh runs on the host, in your login shell, to reach the engine, such as `"perchd bridge"`; with none the app sends the `perchd` it carries there, starts it, and replaces an engine of another build it finds running idle |
| `[lsp] max_memory_mb` | `8192` | Memory all language servers may use before idle ones are stopped |
| `[[knowledge]] dir` | required per entry | External directory relative to `$PERCH_HOME/knowledge/` |
| `[[knowledge]] repos` | every repository | Repository identities that get the directory |
| `[[repo]] github` | none | The repository's `owner/repo` on GitHub; leave out for a local-only repository |
| `[[repo]] name` | the repo's name | Name shown in the sidebar |
| `[[repo]] root` | none | Your local clone, where Perch finds branches and worktrees and cuts new ones |
| `[[repo]] base` | the remote's default | Base branch, when the remote doesn't say |
| `[repo.work] write` | `[]` | Extra directories Work runs may write, such as a package cache |
| `[repo.work] network` | `[]` | Hosts Work runs may reach, such as `index.crates.io` |
| `[repo.work] approvals` | `[work] approvals` | Overrides it for this repository; also takes `"never"`, which allows every request |
| `[repo.work] trust` | `false` | Load the repository's own `.claude/`, `.mcp.json`, `.codex/` and `.agents/` config for Work runs |
| `[repo.lsp] trust` | `false` | Start rust-analyzer and jdtls on anything in this repository without asking |

The [reference](reference.md) covers what each setting does in more detail.

## External knowledge

Add directories in **Settings → Knowledge** or in the engine's
`config.toml`.

```toml
[[knowledge]]
dir = "discord/slatedb"
repos = ["slatedb/slatedb", "slatedb/slatedb-go"]

[[knowledge]]
dir = "notes"
```

Perch matches repository identities case-insensitively. Use `local/<name>`
for a local-only repository. An omitted `repos` applies to every
repository. An empty array applies to none. In Settings, the **All
repositories** switch controls this. Turning it off starts from no
repositories, and clearing the list leaves the directory shared with
none.

Write Markdown into that directory on the engine's machine. Use
`INDEX.md` as an optional entry point and include links back to original
messages or documents where possible. Use ordinary directories and files
with no symlinks or agent instruction files such as `AGENTS.md`,
`CLAUDE.md` and `GEMINI.md`. The top-level names `perch` and `reviews`
are reserved.

Perch shares each configured directory and its contents with eligible
Ask, Work and Review runs as untrusted context. An external ingester
handles fetching, updates, deletions and scheduling. Start a new agent
session to pick up newly configured directories.

Perch skips a directory that is missing, contains a symlink or an agent
instruction file, or holds more than 200,000 files and directories. It
also skips one the agent backend cannot add. Settings shows why a
directory is not shared, and the engine log records each change.
Perch reuses a directory's check for up to a minute, so a fix appears
at the first run after that.

## OpenCode

Perch discovers OpenCode models on the machine running the engine. Select one
in the model picker for Ask or Work, or enable OpenCode in the Review dialog.
You can pin a shorter list under **Settings → Agents → OpenCode**.

Perch gives OpenCode a separate profile so saved terminal permissions cannot
silently approve Work commands. To sign in to a provider for Perch, run this
on the engine machine and restart Perch to refresh its model list.

```sh
perch_home="${PERCH_HOME:-$HOME/.perch}"
OPENCODE_DB="$perch_home/opencode/opencode.db" OPENCODE_CONFIG_DIR="$perch_home/opencode/config" opencode auth login --standalone
```

Custom provider configuration goes in `$perch_home/opencode/config/opencode.json`.
OpenCode Zen also accepts `OPENCODE_API_KEY` in the engine's environment.

Ask and Review have read and search tools. Work allows worktree edits and sends
shell commands through Perch's approval controls. Perch disables project
configuration, plugins and MCP servers for these sessions. When the model
offers the selected effort variant, Perch uses it. Otherwise OpenCode uses
the model's default.
