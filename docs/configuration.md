# Configuration

Perch reads `~/.perch/config.toml`, or `$PERCH_HOME/config.toml` when
`PERCH_HOME` is set. Every setting is optional.

`PERCH_HOME` is also where the cache, the checkouts and the logs live, and
only one engine may use it at a time: a second Perch, or a `perchd`, on the
same home refuses to start and says which process has it (see
[One engine to a Perch home](reference.md#one-engine-to-a-perch-home)).
`perchd` reads the config.toml of the machine it runs on.

`PERCH_SSH`, in the app's environment, names the ssh program the app runs
to reach a host, install `perchd` there and open a terminal on it, in
place of `ssh`: a wrapper that adds `-F <config>`, for tests.

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
| `[work] branch_prefix` | `""` | Put before branch names Perch fills in, such as `"chris/"` |
| `[work] approvals` | `"auto"` | Who answers a Work run's requests outside its sandbox: `"auto"` (the CLI's auto-reviewer, then you) or `"ask"` (you) |
| `[work] approval_timeout_mins` | `15` in the app, `480` under `perchd` | Minutes a request to you waits before it is denied |
| `[keybindings]` *action* | the action's keys | Keys for an action by its id: a string such as `"super+t"`, a list of them, or `""` for none; see [Keyboard shortcuts](reference.md#keyboard-shortcuts) |
| `[ui] appearance` | `"system"` | `"light"`, `"dark"` or `"system"` |
| `[ui] code_font` | none | Code font off macOS; on macOS code is always SF Mono; applies after a restart |
| `[display] zoom` | `1.0` | Scales the whole window, `0.75` to `2.0`; applies after a restart |
| `[display] code_font_size` | `12` | Code font size before zoom, `8` to `32`; applies after a restart |
| `[remote] host` | none | An SSH host whose `perchd` the app uses instead of an engine of its own; applies after a restart |
| `[remote] command` | none | What ssh runs on the host, in your login shell, to reach the engine, such as `"perchd bridge"`; with none the app installs its own `perchd` there, runs that, and replaces an older engine it finds running |
| `[remote] perchd` | none | A Linux `perchd`, or a release tarball of one, on the Mac, for the app to install on the host instead of downloading the release's |
| `[lsp] max_memory_mb` | `8192` | Memory all language servers may use before idle ones are stopped |
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
