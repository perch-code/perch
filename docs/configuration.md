# Configuration

Perch reads `~/.perch/config.toml`, or `$PERCH_HOME/config.toml` when
`PERCH_HOME` is set. Every setting is optional.

| Key | Default | What it does |
|---|---|---|
| `token` | none | GitHub token, used only when the env vars and `gh` have none |
| `poll_interval_secs` | `60` | Shortest time between inbox polls; GitHub can ask for longer |
| `github_host` | none | A GitHub Enterprise host whose remotes count as GitHub |
| `[agent] binary` | `"claude"` | Claude Code executable, a name on `PATH` or a path |
| `[agent] model` | `"opus"` | Model the composer's picker starts on |
| `[agent] effort` | `"high"` | Effort the picker starts on: `low`, `medium`, `high`, `xhigh`, `max` or `ultra` |
| `[agent] models` | `["fable", "opus", "sonnet", "haiku"]` | Claude models the picker offers |
| `[agent] max_concurrent_runs` | `4` | Agents running at once; the rest queue |
| `[agent.codex] binary` | `"codex"` | Codex executable, a name on `PATH` or a path |
| `[agent.codex] models` | Codex's own list | Codex models the picker offers |
| `[work] branch_prefix` | `""` | Put before branch names Perch fills in, such as `"chris/"` |
| `[work] approvals` | `"auto"` | Who answers a Work run's requests outside its sandbox: `"auto"` (the CLI's auto-reviewer, then you) or `"ask"` (you) |
| `[ui] appearance` | `"system"` | `"light"`, `"dark"` or `"system"` |
| `[ui] code_font` | none | Code font off macOS; on macOS code is always SF Mono |
| `[display] zoom` | `1.0` | Scales the whole window, `0.75` to `2.0`; read at startup |
| `[display] code_font_size` | `12` | Code font size before zoom, `8` to `32`; read at startup |
| `[lsp] max_memory_mb` | `8192` | Memory all language servers may use before idle ones are stopped |
| `[[repo]] github` | none | The repository's `owner/repo` on GitHub; leave out for a local-only repository |
| `[[repo]] name` | the repo's name | Name shown in the sidebar |
| `[[repo]] root` | none | Your local clone, where Perch finds branches and worktrees and cuts new ones |
| `[[repo]] base` | the remote's default | Base branch, when the remote doesn't say |
| `[repo.work] write` | `[]` | Extra directories Work runs may write, such as a package cache |
| `[repo.work] network` | `[]` | Hosts Work runs may reach, such as `index.crates.io` |
| `[repo.work] approvals` | `[work] approvals` | Overrides it for this repository; also takes `"never"`, which allows every request |
| `[repo.work] trust` | `false` | Load the repository's own `.claude/`, `.mcp.json` and `.codex/` config for Work runs |
| `[repo.lsp] trust` | `false` | Start rust-analyzer and jdtls on anything in this repository without asking |

The [reference](reference.md) covers what each setting does in more detail.
