//! Gemini, through the Antigravity CLI (`agy`).
//!
//! Two ways in. A question, a review and a verifier run in print mode,
//! `agy --output-format stream-json`, which takes its settings for the
//! run alone with `--settings`: a terminal sandbox, the worktree read-only
//! and no network. Print mode never asks; whatever its settings do not
//! allow is refused at its edge.
//!
//! A Work run goes through `agy acp`, an Agent Client Protocol server,
//! because that is where agy asks: every file write and every command
//! comes to the client as `session/request_permission`. It takes no
//! settings and no sandbox Perch can shape, so Perch is the policy: an
//! edit inside the worktree, clear of the protected paths, is allowed
//! without asking; an edit to a protected path is refused outright; and
//! everything else comes to the reader. The session stays in agy's
//! `default` mode. Its `auto` mode approves edits and sandboxed commands
//! itself, `.agents/hooks.json` among them, without a request reaching
//! Perch.
//!
//! Both load the checkout's own `.agents/` (and `.agent/`, `_agents/`,
//! `_agent/`), which can hold hooks, MCP servers and plugins that run as
//! the reader, outside any sandbox, with no trust prompt and no flag to
//! turn them off. So a checkout whose configuration holds anything but
//! skills, rules and Markdown workflows is refused unless the reader
//! trusts the repository; see [`refusal`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::interactive::{ApprovalKind, ApprovalRequest, Decision, Reply};
use crate::parse::{counts_suffix, one_line, short_path, str_of};
use crate::RunEvent;

/// Where agy looks for a workspace's own customizations: hooks, MCP
/// servers, plugins, agents, sidecars, skills, rules and workflows. agy
/// reads all four in every directory from a workspace up to the top of
/// its repository.
pub const CONFIG_DIRS: &[&str] = &[".agents", ".agent", "_agents", "_agent"];

/// What marks the top of a repository for agy, in the order it looks.
/// Without one, agy reads its workspace alone and nothing above it.
const VCS_MARKERS: &[&str] = &[".git", ".hg", ".jj", ".repo", ".citc"];

/// How many entries of one configuration directory Perch looks at, how
/// deep, and how large a Markdown file it reads for includes, before it
/// gives up and refuses.
const SCAN_LIMIT: usize = 5_000;
const DEPTH_LIMIT: usize = 16;
const READ_LIMIT: u64 = 1 << 20;

/// Files a folder picks up without anyone putting configuration in it,
/// which agy loads nothing from.
const IGNORED: &[&str] = &[".DS_Store", ".gitkeep", ".gitignore"];

/// Names that load hooks, MCP servers, plugins, agents or sidecars at the
/// top of a configuration directory. agy 1.5.9 ignores them inside a
/// skill, but Perch refuses them there too, in any case, so a loader that
/// starts looking deeper fails closed.
const LOADER_NAMES: &[&str] = &[
    "hooks.json",
    "mcp_config.json",
    "mcp.json",
    ".mcp.json",
    "plugin.json",
    "plugins.json",
    "plugins.txt",
    "agents.json",
    "agents.txt",
    "skills.json",
    "skills.txt",
    "rules.json",
    "workflows.json",
    "workflow.json",
    "sidecar.json",
    "sidecars.json",
    ".agents",
    ".agent",
    "_agents",
    "_agent",
    ".gemini",
    ".antigravity-plugin",
    ".gemini-plugin",
    ".claude-plugin",
];

/// What makes Perch refuse a checkout: a path, and a sentence about it
/// with `{path}` where the path goes.
struct Finding {
    path: PathBuf,
    says: String,
}

impl Finding {
    fn new(path: &Path, says: impl Into<String>) -> Finding {
        Finding {
            path: path.to_path_buf(),
            says: says.into(),
        }
    }
}

/// The directories a print-mode run adds to its workspace beside `cwd`:
/// `dirs`, then a linked worktree's git directories and the object
/// directories it borrows, which git needs and agy's sandbox reads
/// nothing outside of. agy takes each as a workspace of its own and
/// loads its configuration too, so [`refusal`] checks them all.
pub(crate) fn print_dirs(cwd: &Path, dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = dirs.to_vec();
    dirs.extend(crate::linked_git_dirs(cwd));
    dirs.extend(crate::alternate_object_dirs(cwd));
    dirs
}

pub(crate) fn can_share_with_work(cwd: &Path, dir: &Path) -> bool {
    let dir = crate::real_path(dir);
    // agy can write here directly even in ACP's default mode, without
    // a permission request Perch could refuse.
    !dir.starts_with(crate::real_path(cwd))
        && !dir.starts_with(crate::real_path(&std::env::temp_dir()))
        && !dir.starts_with(crate::real_path(Path::new("/tmp")))
}

/// Why a Gemini run in `cwd`, adding `added` to its workspace, will not
/// start, or `None` when it may.
///
/// agy loads each workspace's configuration from every directory between
/// it and the top of its repository: the [`CONFIG_DIRS`], and a
/// `.gemini/config.json` that can name another directory to load them
/// from. What runs code (hooks, MCP servers, plugins, agents, sidecars)
/// runs it as the reader, outside any sandbox. So Perch accepts a
/// configuration directory only when everything in it is known not to:
/// skills, rules, Markdown workflows and a README. Anything else, a name
/// it does not know among them, is refused; so is a rule, `AGENTS.md` or
/// `GEMINI.md` that brings a file from outside the repository into the
/// prompt, which agy reads itself, outside the sandbox, and a review
/// could then quote back.
pub(crate) fn refusal(cwd: &Path, added: &[PathBuf], trusted: bool) -> Option<String> {
    if trusted {
        return None;
    }
    let mut seen = HashSet::new();
    let starts = std::iter::once((cwd, true)).chain(added.iter().map(|d| (d.as_path(), false)));
    for (start, own) in starts {
        let dirs = walked_dirs(start);
        let root = dirs.last().cloned().unwrap_or_else(|| start.to_path_buf());
        for dir in dirs {
            if !seen.insert(dir.clone()) {
                continue;
            }
            if let Some(finding) = vet_dir(&dir, &root) {
                return Some(refusal_text(cwd, &finding, own));
            }
        }
    }
    None
}

/// The refusal as the reader sees it. Trust lets Work run, and Work
/// loads only its own checkout's configuration: when what Perch found
/// came from a directory a question or review adds, Work needs nothing.
fn refusal_text(cwd: &Path, finding: &Finding, own: bool) -> String {
    let real = crate::real_path(cwd);
    let shown = match finding.path.strip_prefix(&real) {
        Ok(inside) if !inside.as_os_str().is_empty() => inside.display().to_string(),
        _ => crate::prompt_path(&finding.path),
    };
    let what = finding.says.replace("{path}", &shown);
    if own {
        format!(
            "{what}. So Perch does not run Gemini here. Gemini can still work here, as Work, once \
the repository is trusted in config.toml (`[repo.work] trust = true`); a question or a review \
can't."
        )
    } else {
        format!(
            "{what}. A question or a review here also reads that directory, so Perch does not run \
Gemini for one. Work reads only this checkout and can still run here."
        )
    }
}

/// Every directory agy reads configuration from for a workspace at
/// `start`: it and each one above it, up to the top of its repository;
/// or `start` alone outside one. The top comes last.
fn walked_dirs(start: &Path) -> Vec<PathBuf> {
    let real = crate::real_path(start);
    let mut dirs = Vec::new();
    for dir in real.ancestors() {
        dirs.push(dir.to_path_buf());
        if VCS_MARKERS
            .iter()
            .any(|marker| dir.join(marker).symlink_metadata().is_ok())
        {
            return dirs;
        }
    }
    vec![real]
}

/// What in `dir` itself makes agy unsafe to run: a configuration
/// directory holding more than Perch accepts, a `.gemini/config.json`,
/// or an `AGENTS.md` or `GEMINI.md` that brings in a file from outside
/// `root`. Names are looked up by joining them on, so on a filesystem
/// that ignores case `.AGENTS` is found as agy finds it.
fn vet_dir(dir: &Path, root: &Path) -> Option<Finding> {
    for name in CONFIG_DIRS {
        if let Some(finding) = vet_config(&dir.join(name), root) {
            return Some(finding);
        }
    }
    let gemini = dir.join(".gemini").join("config.json");
    if gemini.symlink_metadata().is_ok() {
        return Some(Finding::new(
            &gemini,
            "{path} can point agy at hooks and MCP servers elsewhere in the repository, which it \
runs as you, outside any sandbox",
        ));
    }
    ["AGENTS.md", "GEMINI.md"]
        .iter()
        .find_map(|name| vet_context_file(&dir.join(name), root))
}

/// `AGENTS.md` or `GEMINI.md`, which agy puts in the prompt whole. A link
/// is fine while it stays in the repository (`AGENTS.md` beside a
/// `CLAUDE.md` it points at, say); its includes are checked from both
/// ends, not knowing which one agy resolves them from.
fn vet_context_file(file: &Path, root: &Path) -> Option<Finding> {
    let meta = file.symlink_metadata().ok()?;
    if !meta.is_symlink() {
        return meta
            .is_file()
            .then(|| vet_includes(file, file, root))
            .flatten();
    }
    let Ok(real) = std::fs::canonicalize(file) else {
        return None;
    };
    if !real.starts_with(root) {
        return Some(Finding::new(
            file,
            "{path} links to a file outside the repository, which agy would put in the prompt",
        ));
    }
    if !real.is_file() {
        return None;
    }
    vet_includes(file, file, root).or_else(|| vet_includes(file, &real, root))
}

/// What in one configuration directory Perch does not accept, if
/// anything. Its own entries are compared exactly, so `Skills/` is not
/// `skills/`, and nothing is followed through a symbolic link.
fn vet_config(config: &Path, root: &Path) -> Option<Finding> {
    let meta = config.symlink_metadata().ok()?;
    if meta.is_symlink() {
        return Some(link(config));
    }
    if !meta.is_dir() {
        return Some(unknown(config));
    }
    let mut budget = SCAN_LIMIT;
    let list = match entries(config, &mut budget) {
        Ok(list) => list,
        Err(finding) => return Some(finding),
    };
    for (path, meta) in list {
        if meta.is_symlink() {
            return Some(link(&path));
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let found = match name.as_ref() {
            "README.md" if meta.is_file() => None,
            "skills" if meta.is_dir() => vet_skills(&path, &mut budget),
            "rules" | "workflows" if meta.is_dir() => vet_markdown(&path, root, &mut budget),
            _ => Some(classify(&path, &name)),
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// A directory's entries with their own metadata, links unfollowed, in
/// name order so the same checkout always gives the same answer; the
/// [`IGNORED`] names left out. Each counts against `budget`, and what
/// can't be listed is a refusal, not an empty directory.
fn entries(dir: &Path, budget: &mut usize) -> Result<Vec<(PathBuf, std::fs::Metadata)>, Finding> {
    let mut list = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|_| unreadable(dir))? {
        if *budget == 0 {
            return Err(too_big(dir));
        }
        *budget -= 1;
        let path = entry.map_err(|_| unreadable(dir))?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if IGNORED.contains(&name.as_ref()) {
            continue;
        }
        let meta = path.symlink_metadata().map_err(|_| unreadable(&path))?;
        list.push((path, meta));
    }
    list.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(list)
}

/// `skills/`: each skill a directory of plain files at any depth, its
/// scripts among them, which run only when the model runs them (in the
/// sandbox, or by asking); or a loose Markdown file.
fn vet_skills(skills: &Path, budget: &mut usize) -> Option<Finding> {
    let list = match entries(skills, budget) {
        Ok(list) => list,
        Err(finding) => return Some(finding),
    };
    for (path, meta) in list {
        if meta.is_symlink() {
            return Some(link(&path));
        }
        let found = if meta.is_dir() {
            vet_skill_tree(&path, 1, budget)
        } else if meta.is_file() && is_markdown(&path) {
            None
        } else {
            Some(unknown(&path))
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn vet_skill_tree(dir: &Path, depth: usize, budget: &mut usize) -> Option<Finding> {
    if depth > DEPTH_LIMIT {
        return Some(too_big(dir));
    }
    let list = match entries(dir, budget) {
        Ok(list) => list,
        Err(finding) => return Some(finding),
    };
    for (path, meta) in list {
        if meta.is_symlink() {
            return Some(link(&path));
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if LOADER_NAMES.iter().any(|n| name.eq_ignore_ascii_case(n)) {
            return Some(Finding::new(
                &path,
                "{path} is named like a file agy loads hooks, MCP servers or plugins from. agy \
ignores it inside a skill today, but Perch refuses it there too",
            ));
        }
        let found = if meta.is_dir() {
            vet_skill_tree(&path, depth + 1, budget)
        } else if meta.is_file() {
            None
        } else {
            Some(unknown(&path))
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// `rules/` and `workflows/`: Markdown files only, directly inside. A
/// directory under `workflows/` is agy's executable kind, a Python flow.
fn vet_markdown(dir: &Path, root: &Path, budget: &mut usize) -> Option<Finding> {
    let list = match entries(dir, budget) {
        Ok(list) => list,
        Err(finding) => return Some(finding),
    };
    list.into_iter().find_map(|(path, meta)| {
        if meta.is_symlink() {
            Some(link(&path))
        } else if meta.is_file() && is_markdown(&path) {
            vet_includes(&path, &path, root)
        } else {
            Some(unknown(&path))
        }
    })
}

fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "md")
}

/// An `@[label](target)` include in `file` that brings in a file from
/// outside `root`. agy reads the target itself, outside the sandbox, and
/// puts it in the prompt, resolving a relative one from the including
/// file's directory (`from`'s); Perch resolves it the same way, through
/// any link, and refuses a `~` or a URL, which it can't. A target that
/// isn't there brings nothing. The file is read as bytes, as agy reads
/// it, so one bad byte doesn't hide what follows.
fn vet_includes(file: &Path, from: &Path, root: &Path) -> Option<Finding> {
    let too_big = std::fs::metadata(from).map_or(true, |meta| meta.len() > READ_LIMIT);
    if too_big {
        return Some(Finding::new(
            file,
            "{path} is too large for Perch to check what agy would include from it",
        ));
    }
    let Ok(bytes) = std::fs::read(from) else {
        return Some(unreadable(file));
    };
    let text = String::from_utf8_lossy(&bytes);
    let base = from.parent().unwrap_or(root);
    let target = includes(&text).into_iter().find(|target| {
        if target.starts_with('~') || target.contains(':') {
            return true;
        }
        std::fs::canonicalize(base.join(target)).is_ok_and(|real| !real.starts_with(root))
    })?;
    Some(Finding::new(
        file,
        format!(
            "{{path}} includes {target}, from outside the repository, which agy would put in the \
prompt"
        ),
    ))
}

/// The targets of every `@[label](target)` in `text`.
fn includes(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("@[") {
        rest = &rest[at + 2..];
        let Some(close) = rest.find(']') else { break };
        let after = &rest[close + 1..];
        if let Some(target) = after.strip_prefix('(') {
            if let Some(end) = target.find(')') {
                let target = target[..end].trim();
                if !target.is_empty() {
                    found.push(target);
                }
            }
        }
    }
    found
}

/// What a name Perch does not accept at the top of a configuration
/// directory would bring, where agy's documentation says.
fn classify(path: &Path, name: &str) -> Finding {
    let name = name.to_ascii_lowercase();
    let says = match name.as_str() {
        "hooks.json" => {
            "{path} defines hooks, which agy runs as you, outside any sandbox, with no way to \
turn them off"
        }
        "mcp_config.json" | "mcp.json" => {
            "{path} defines MCP servers, which agy starts as you, outside any sandbox, with no way \
to turn them off"
        }
        "plugins" | "plugins.json" | "plugins.txt" => {
            "{path} adds plugins, which bring hooks and MCP servers that agy runs as you, outside \
any sandbox"
        }
        "agents" | "agents.json" | "agents.txt" => {
            "{path} adds custom agents, which can bring hooks and MCP servers that agy runs as \
you, outside any sandbox"
        }
        "sidecars" | "sidecars.json" => {
            "{path} defines sidecars, background commands that agy runs as you, outside any \
sandbox"
        }
        _ => return unknown(path),
    };
    Finding::new(path, says)
}

fn unknown(path: &Path) -> Finding {
    Finding::new(
        path,
        "{path} is in agy's configuration, and Perch can't vouch for it: it accepts only skills, \
rules, Markdown workflows and a README there, since agy runs hooks, MCP servers and plugins from \
that folder as you, outside any sandbox",
    )
}

fn link(path: &Path) -> Finding {
    Finding::new(
        path,
        "{path} is a symbolic link in agy's configuration, which Perch does not follow to check \
what agy would load through it",
    )
}

fn unreadable(path: &Path) -> Finding {
    Finding::new(
        path,
        "Perch can't read {path} to check what agy would load from it",
    )
}

fn too_big(path: &Path) -> Finding {
    Finding::new(path, "{path} is too large or too deep for Perch to check")
}

/// The effort as agy spells it. agy's levels stop at `max`; Ultra, which
/// for Claude Code and Codex adds delegation, asks for its deepest level.
pub(crate) fn effort_name(effort: crate::Effort) -> &'static str {
    match effort {
        crate::Effort::Ultra => "max",
        other => other.name(),
    }
}

/// ACP's thinking level, which has only `low`, `medium` and `high`.
fn thought_level(effort: &str) -> &'static str {
    match effort {
        "low" => "low",
        "medium" => "medium",
        _ => "high",
    }
}

/// The `--settings` a sandboxed print-mode run starts with. They outrank the
/// user's file for the run and are never saved. The terminal sandbox is
/// on and commands run in it without asking; the worktree and `dirs` are
/// mounted read-only by a deny rule, which outranks any allow; the
/// sandbox has no network; and web reads, browser use and MCP tools are
/// denied by name, since they run outside the sandbox. Anything else that
/// would ask (including an unsandboxed command) is refused, print mode
/// having nobody to ask.
pub fn read_only_settings(cwd: &Path, dirs: &[PathBuf]) -> String {
    let deny_write = std::iter::once(cwd)
        .chain(dirs.iter().map(PathBuf::as_path))
        .map(|path| format!("write_file({})", crate::real_path(path).display()));
    let deny: Vec<String> = deny_write
        .chain(
            ["read_url(*)", "execute_url(*)", "mcp(*)"]
                .into_iter()
                .map(str::to_string),
        )
        .collect();
    json!({
        "agentMode": "",
        "toolPermission": "proceed-in-sandbox",
        "enableTerminalSandbox": true,
        "allowNonWorkspaceAccess": false,
        "permissions": {"allow": [], "ask": [], "deny": deny},
    })
    .to_string()
}

/// Read-only git subcommands allowed when `[agent] sandbox = false` turns
/// agy's terminal sandbox off.
const UNSANDBOXED_GIT_SUBCOMMANDS: &[&str] = &[
    "diff",
    "log",
    "show",
    "blame",
    "status",
    "rev-parse",
    "merge-base",
    "ls-files",
    "grep",
    "ls-tree",
    "cat-file",
];

/// Read-only inspection commands allowed alongside git when `[agent]
/// sandbox = false`. agy's shell parser still refuses output redirection
/// and command substitution on allowed commands, and print mode aborts
/// the turn on any unlisted command (such as `echo` chained between git
/// calls or `ls` on a context folder). `rg`, `find`, and `sed` are left
/// out (`rg --pre` and `find -exec` run arbitrary programs, `sed -i`
/// edits in place); agy's built-in file tools cover those instead.
const UNSANDBOXED_SHELL_COMMANDS: &[&str] =
    &["ls", "cat", "head", "tail", "wc", "echo", "pwd", "grep"];

/// Added to a print-mode prompt when `[agent] sandbox = false` so Gemini
/// stays inside the allowed command list instead of triggering an
/// auto-denied shell command that aborts the turn.
pub(crate) const UNSANDBOXED_SHELL_NOTE: &str = "Shell commands are restricted to read-only git (git diff, git log, git show, git blame, git status, git rev-parse, git merge-base, git ls-files, git grep, git ls-tree, git cat-file) and ls, cat, head, tail, wc, echo, pwd, grep, with no redirections or command substitutions. Use your built-in file tools (read_file, list_dir, find_by_name, grep_search) rather than other shell commands such as find, rg, or sed — an unlisted shell command aborts the run.";

/// The `--settings` a print-mode run starts with when `[agent] sandbox =
/// false`: agy's terminal sandbox is off, `toolPermission` is
/// `request-review`, file writes and web/MCP tools are denied, and only
/// read-only git and inspection commands are allowed. `agy`'s `command(...)`
/// rules match literal space-separated token prefixes, so `--output` is
/// denied immediately after the git subcommand while `--output=...` or
/// later flags cannot be matched by prefix; as with Claude Code, turning
/// the sandbox off is only for a machine that is itself the boundary.
pub fn unsandboxed_read_only_settings(cwd: &Path, dirs: &[PathBuf]) -> String {
    let real_cwd = crate::real_path(cwd);
    let cwd_display = real_cwd.display();
    let allow: Vec<String> = UNSANDBOXED_GIT_SUBCOMMANDS
        .iter()
        .flat_map(|sub| {
            [
                format!("command(git {sub})"),
                format!("command(git -C {cwd_display} {sub})"),
            ]
        })
        .chain(
            UNSANDBOXED_SHELL_COMMANDS
                .iter()
                .map(|cmd| format!("command({cmd})")),
        )
        .collect();
    let deny_write = std::iter::once("write_file(*)".to_string()).chain(
        std::iter::once(cwd)
            .chain(dirs.iter().map(PathBuf::as_path))
            .map(|path| format!("write_file({})", crate::real_path(path).display())),
    );
    let deny_output = ["diff", "log", "show"].into_iter().flat_map(|sub| {
        [
            format!("command(git {sub} --output)"),
            format!("command(git -C {cwd_display} {sub} --output)"),
        ]
    });
    let deny: Vec<String> = deny_write
        .chain(
            ["read_url(*)", "execute_url(*)", "mcp(*)"]
                .into_iter()
                .map(str::to_string),
        )
        .chain(deny_output)
        .collect();
    json!({
        "agentMode": "",
        "toolPermission": "request-review",
        "enableTerminalSandbox": false,
        "allowNonWorkspaceAccess": false,
        "permissions": {"allow": allow, "ask": [], "deny": deny},
    })
    .to_string()
}

/// The arguments every print-mode run shares, before the model's.
///
/// A linked worktree's git directories sit outside it, where agy's
/// sandbox reads nothing, and git there says "not a git repository"; so
/// do the object directories a checkout borrows through alternates, a
/// review snapshot's among them, without which git can't open a commit.
/// They go in read-only with the rest.
pub(crate) fn print_args(
    cwd: &Path,
    dirs: &[PathBuf],
    sandbox: bool,
    model: &str,
    effort: &str,
) -> Vec<String> {
    let dirs = print_dirs(cwd, dirs);
    let settings = if sandbox {
        read_only_settings(cwd, &dirs)
    } else {
        unsandboxed_read_only_settings(cwd, &dirs)
    };
    let mut args: Vec<String> = vec![
        "--output-format".into(),
        "stream-json".into(),
        "--disable-slash-commands".into(),
        "--settings".into(),
        settings,
        "--model".into(),
        model.to_string(),
        "--effort".into(),
        effort.to_string(),
    ];
    for dir in &dirs {
        args.push("--add-dir".into());
        args.push(crate::real_path(dir).to_string_lossy().into_owned());
    }
    args
}

/// A JSON schema as Gemini's API takes it: it refuses a `null` in an
/// `enum` ("cannot be empty"), so those go. A `"type": [.., "null"]`
/// beside it still allows the null.
pub(crate) fn api_schema(schema: &str) -> String {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map.iter_mut() {
                    match inner {
                        Value::Array(items) if key == "enum" => items.retain(|v| !v.is_null()),
                        _ => strip(inner),
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    match serde_json::from_str::<Value>(schema) {
        Ok(mut value) => {
            strip(&mut value);
            value.to_string()
        }
        Err(_) => schema.to_string(),
    }
}

/// What a step does, for the reader: agy's tool names and argument keys,
/// which are its own (`run_command` with `CommandLine`).
pub fn summarize(name: &str, args: &Value) -> String {
    let field = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| args.get(*k).and_then(Value::as_str))
            .map(str::to_string)
    };
    let path = || {
        field(&[
            "Path",
            "AbsolutePath",
            "TargetFile",
            "File",
            "FilePath",
            "DirectoryPath",
            "SearchPath",
            "SearchDirectory",
            "path",
            "filePath",
        ])
        .map(|p| short_path(&p))
    };
    let summary = match name {
        "shell" => format!("Ran {}", field(&["command"]).unwrap_or_default()),
        "run_command" => format!(
            "Ran {}",
            field(&["CommandLine", "Command"]).unwrap_or_default()
        ),
        "read" | "view_file" | "read_file" | "view_file_outline" | "view_code_item" => {
            format!("Read {}", path().unwrap_or_default())
        }
        "list_dir" => format!("Listed {}", path().unwrap_or_else(|| ".".into())),
        "find_by_name" | "glob" => format!(
            "Listed {}",
            field(&["Pattern", "pattern"]).unwrap_or_default()
        ),
        "grep_search" | "grep" => {
            let query = field(&["Query", "Pattern", "pattern"]).unwrap_or_default();
            match path() {
                Some(p) => format!("Searched for {query} in {p}"),
                None => format!("Searched for {query}"),
            }
        }
        "write"
        | "edit"
        | "write_file"
        | "write_to_file"
        | "replace_file_content"
        | "multi_replace_file_content"
        | "sed_file"
        | "edit_file"
        | "notebook_edit" => {
            let what = path().unwrap_or_default();
            match field(&["Content", "CodeContent"]) {
                Some(content) if matches!(name, "write_file" | "write_to_file") => format!(
                    "Edited {what} {}",
                    counts_suffix(content.lines().count(), 0)
                ),
                _ => format!("Edited {what}"),
            }
        }
        "search_web" => format!(
            "Searched the web for {}",
            field(&["query", "Query"]).unwrap_or_default()
        ),
        "read_url_content" => format!("Read {}", field(&["Url", "url"]).unwrap_or_default()),
        other => match path() {
            Some(p) => format!("{other} {p}"),
            None => other.to_string(),
        },
    };
    one_line(&summary, 120)
}

/// The first line of a failed step's error, or of its output.
fn error_line(info: &Value) -> String {
    let message = info
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("");
    one_line(message, 100)
}

/// A command step that ran says how it exited (`Command exited with code
/// N.`), and one that could not start says why (`Command failed to run:
/// ...`, e.g. when agy's sandbox cannot connect). In print mode an
/// auto-denied command can also finish `DONE` with empty output and no
/// error object, so an empty output is treated as failed too.
fn command_failed(output: &str) -> Option<String> {
    if let Some(code) = output
        .lines()
        .find_map(|l| l.trim().strip_prefix("Command exited with code "))
    {
        let code = code.trim_end_matches('.').trim();
        return (code != "0").then(|| {
            output
                .lines()
                .rev()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(|l| one_line(l, 100))
                .unwrap_or_default()
        });
    }
    if let Some(line) = output
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("Command failed to run:"))
    {
        return Some(one_line(line, 100));
    }
    output
        .trim()
        .is_empty()
        .then(|| "command did not run".to_string())
}

fn is_sandbox_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("sandbox server") || lower.contains("sbox:")
}

/// Whether a step's state is one it ended in without finishing.
fn step_errored(state: &str) -> bool {
    matches!(
        state,
        "ERROR" | "INVALID" | "CANCELED" | "CANCELLED" | "INTERRUPTED" | "HALTED"
    )
}

/// A tool step's name, from the step or its `tool_info`.
fn step_tool_name(s: &Value, info: &Value) -> String {
    match str_of(s, "tool_name") {
        n if n.is_empty() => str_of(info, "name"),
        n => n,
    }
}

/// Why a finished tool step failed, or `None` if it did not.
fn tool_failure(name: &str, info: &Value, state: &str) -> Option<String> {
    if info.get("error").is_some_and(|e| !e.is_null()) {
        Some(error_line(info))
    } else if step_errored(state) {
        Some(state.to_ascii_lowercase())
    } else if name == "run_command" {
        command_failed(&str_of(info, "output"))
    } else {
        None
    }
}

/// The sandbox startup failure a finished `run_command` step ended in.
fn sandbox_failure(name: &str, failure: Option<&str>) -> Option<String> {
    failure
        .filter(|msg| name == "run_command" && is_sandbox_error(msg))
        .map(str::to_string)
}

/// A `SUCCESS` result that still failed the turn: agy answered nothing
/// after a permission denial or a sandbox that could not start. The
/// parser and the print host both judge a turn by this.
fn result_failed_without_output(r: &Value, sandbox_error: bool) -> bool {
    let has_structured = r.get("structured_output").is_some_and(|v| !v.is_null());
    let has_response = !str_of(r, "response").trim().is_empty();
    let has_denied = r
        .get("denied_actions")
        .and_then(Value::as_array)
        .is_some_and(|actions| !actions.is_empty());
    !has_structured && !has_response && (sandbox_error || has_denied)
}

// ---------------------------------------------------------------------
// Print mode
// ---------------------------------------------------------------------

/// `agy --output-format stream-json`: an `init`, then a `step_update` for
/// each step as it goes and ends, and a `result` for each turn.
pub struct GeminiParser {
    session_id: String,
    model: String,
    /// The text of each response step as it streams, by its subagent's
    /// parent step (`None` for the run's own) and its index.
    texts: HashMap<(Option<u64>, u64), String>,
    /// Tool steps that have begun, and those whose result has been emitted.
    started: HashSet<(Option<u64>, u64)>,
    ended: HashSet<(Option<u64>, u64)>,
    /// First sandbox startup failure seen on a command in the current turn.
    sandbox_error: Option<String>,
}

impl Default for GeminiParser {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiParser {
    pub fn new() -> Self {
        Self {
            session_id: String::new(),
            model: String::new(),
            texts: HashMap::new(),
            started: HashSet::new(),
            ended: HashSet::new(),
            sandbox_error: None,
        }
    }

    pub fn parse_line(&mut self, line: &str) -> Vec<RunEvent> {
        match serde_json::from_str::<Value>(line) {
            Ok(v) => self.parse_value(&v),
            Err(_) => Vec::new(),
        }
    }

    pub(crate) fn parse_value(&mut self, v: &Value) -> Vec<RunEvent> {
        match v.get("event").and_then(Value::as_str) {
            Some("init") => {
                self.session_id = str_of(v, "conversation_id");
                let init = v.get("init").unwrap_or(&Value::Null);
                self.model = str_of(init, "model");
                vec![RunEvent::Started {
                    session_id: self.session_id.clone(),
                    model: self.model.clone(),
                }]
            }
            Some("step_update") => self.step(v.get("step_update").unwrap_or(&Value::Null)),
            Some("result") => self.result(v.get("result").unwrap_or(&Value::Null)),
            _ => Vec::new(),
        }
    }

    fn step(&mut self, s: &Value) -> Vec<RunEvent> {
        let index = s.get("step_index").and_then(Value::as_u64).unwrap_or(0);
        let state = str_of(s, "state");
        let done = state == "DONE" || step_errored(&state);
        let parent = s
            .pointer("/subagent_info/parent_step_index")
            .and_then(Value::as_u64);
        let parent_id = parent.map(|i| format!("step-{i}"));
        // A subagent counts its own steps from zero, so its are kept
        // apart from the run's, and what it says is its parent's to use,
        // not the answer.
        let key = (parent, index);
        match str_of(s, "step_type").as_str() {
            "agent_response" => {
                let text = self.texts.entry(key).or_default();
                text.push_str(&str_of(s, "text_delta"));
                if !done {
                    return Vec::new();
                }
                let text = self.texts.remove(&key).unwrap_or_default();
                if text.trim().is_empty() || parent.is_some() {
                    Vec::new()
                } else {
                    vec![RunEvent::Text(text.trim_end().to_string())]
                }
            }
            "tool" => {
                let info = s.get("tool_info").cloned().unwrap_or(Value::Null);
                let name = step_tool_name(s, &info);
                let id = match parent {
                    Some(p) => format!("step-{p}-{index}"),
                    None => format!("step-{index}"),
                };
                let call = RunEvent::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    summary: summarize(&name, info.get("parameters").unwrap_or(&Value::Null)),
                    parent_id,
                };
                let mut events = Vec::new();
                if self.started.insert(key) {
                    events.push(call);
                }
                if done && self.ended.insert(key) {
                    let failure = tool_failure(&name, &info, &state);
                    if let Some(msg) = sandbox_failure(&name, failure.as_deref()) {
                        self.sandbox_error.get_or_insert(msg);
                    }
                    events.push(RunEvent::ToolResult {
                        id,
                        ok: failure.is_none(),
                        summary: failure.unwrap_or_default(),
                    });
                }
                events
            }
            _ => Vec::new(),
        }
    }

    fn result(&mut self, r: &Value) -> Vec<RunEvent> {
        let sandbox_error = self.sandbox_error.take();
        let mut events = Vec::new();
        // A structured result is the run's answer: what a review is read
        // from. It goes as the last thing said, fenced, so the review's
        // reader finds it after whatever prose came first.
        if let Some(structured) = r.get("structured_output").filter(|v| !v.is_null()) {
            events.push(RunEvent::Text(format!(
                "```json\n{}\n```",
                serde_json::to_string_pretty(structured).unwrap_or_default()
            )));
        }
        // What agy refused without a step to show for it, like a read
        // outside the workspace. Without these a turn can end in silence.
        let mut denied_tools = Vec::new();
        for denied in r
            .get("denied_actions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let tool = match str_of(denied, "display_name") {
                name if name.is_empty() => str_of(denied, "action"),
                name => name,
            };
            if !tool.is_empty() && !denied_tools.contains(&tool) {
                denied_tools.push(tool.clone());
            }
            events.push(RunEvent::PermissionDenied {
                tool: (!tool.is_empty()).then_some(tool),
                message: "agy did not allow it".into(),
            });
        }
        if let Some(usage) = r.get("usage") {
            events.push(RunEvent::Usage {
                input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
                output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
                cached_input_tokens: usage.get("cache_read_tokens").and_then(Value::as_u64),
            });
        }
        let session_id = match str_of(r, "conversation_id") {
            id if id.is_empty() => self.session_id.clone(),
            id => id,
        };
        let duration_ms = r
            .get("duration_seconds")
            .and_then(Value::as_f64)
            .map(|s| (s * 1000.0) as u64)
            .unwrap_or(0);
        match str_of(r, "status").as_str() {
            // Headless `agy` still exits 0 with status "SUCCESS" and an empty
            // response when a permission denial (including an unsandboxed
            // retry after the sandbox failed) aborts the turn. Treat that as a
            // failure so the run does not pass as completed with nothing found.
            "SUCCESS" if result_failed_without_output(r, sandbox_error.is_some()) => {
                let message = if sandbox_error.is_some() {
                    "Gemini's (agy) terminal sandbox could not start, so the run stopped without answering. On a container or throwaway VM where the machine is itself the boundary, turn Gemini's sandbox off in Settings > Agents > Gemini (Antigravity) ([agent.gemini] sandbox = false).".to_string()
                } else if denied_tools.is_empty() {
                    "agy stopped without answering because a tool permission was denied".to_string()
                } else {
                    format!(
                        "agy stopped without answering because it did not allow {}",
                        denied_tools.join(", ")
                    )
                };
                events.push(RunEvent::Failed { message });
            }
            "SUCCESS" => events.push(RunEvent::Finished {
                session_id,
                cost_usd: None,
                duration_ms,
            }),
            status => {
                let error = str_of(r, "error");
                events.push(RunEvent::Failed {
                    message: if error.trim().is_empty() {
                        format!("agy ended the turn as {}", status.to_ascii_lowercase())
                    } else {
                        one_line(&error, 300)
                    },
                })
            }
        }
        events
    }
}

/// A print-mode run held on stdin, `--input-format stream-json`: the
/// prompt is the first user message, and the reader's steers wait for
/// the turn to end, then go as the next. agy reads each as a new turn
/// and says so with a `user_input` step.
pub(crate) struct GeminiHost {
    prompt: String,
    /// Steers not yet sent, oldest first.
    queued: VecDeque<String>,
    /// Steers sent and not yet read.
    sent: VecDeque<String>,
    /// `user_input` steps seen; the first is the prompt.
    inputs: usize,
    /// Whether a command in the current turn failed because agy's
    /// sandbox could not start, which the parser fails the turn on.
    sandbox_error: bool,
}

impl GeminiHost {
    pub fn new(prompt: String) -> Self {
        Self {
            prompt,
            queued: VecDeque::new(),
            sent: VecDeque::new(),
            inputs: 0,
            sandbox_error: false,
        }
    }

    pub fn opening(&mut self) -> Vec<String> {
        vec![user_message(&self.prompt)]
    }

    pub fn steer(&mut self, text: &str) -> Vec<String> {
        self.queued.push_back(text.to_string());
        Vec::new()
    }

    pub fn after_line(&mut self, v: &Value) -> Reply {
        match v.get("event").and_then(Value::as_str) {
            Some("step_update") => {
                let s = v.get("step_update").unwrap_or(&Value::Null);
                let state = str_of(s, "state");
                if str_of(s, "step_type") == "tool" && (state == "DONE" || step_errored(&state)) {
                    let info = s.get("tool_info").unwrap_or(&Value::Null);
                    let name = step_tool_name(s, info);
                    let failure = tool_failure(&name, info, &state);
                    self.sandbox_error |= sandbox_failure(&name, failure.as_deref()).is_some();
                    return Reply::default();
                }
                if str_of(s, "step_type") != "user_input"
                    || str_of(s, "state") != "DONE"
                    || s.get("subagent_info").is_some_and(|i| !i.is_null())
                {
                    return Reply::default();
                }
                self.inputs += 1;
                if self.inputs == 1 {
                    return Reply::default();
                }
                Reply {
                    events: self
                        .sent
                        .pop_front()
                        .map(RunEvent::Steered)
                        .into_iter()
                        .collect(),
                    ..Reply::default()
                }
            }
            Some("result") => {
                let result = v.get("result").unwrap_or(&Value::Null);
                let sandbox_error = std::mem::take(&mut self.sandbox_error);
                let failed = str_of(result, "status") != "SUCCESS"
                    || result_failed_without_output(result, sandbox_error);
                match self.queued.pop_front() {
                    Some(next) if !failed => {
                        let write = user_message(&next);
                        self.sent.push_back(next);
                        Reply {
                            writes: vec![write],
                            more: true,
                            ..Reply::default()
                        }
                    }
                    _ => Reply {
                        close: true,
                        ..Reply::default()
                    },
                }
            }
            _ => Reply::default(),
        }
    }
}

fn user_message(text: &str) -> String {
    json!({"event": "user", "message": {"role": "user", "content": text}}).to_string()
}

// ---------------------------------------------------------------------
// ACP, for Work
// ---------------------------------------------------------------------

/// What a Work session starts with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AcpSession {
    pub backend: crate::Backend,
    pub cwd: PathBuf,
    pub model: String,
    /// agy's name for the effort; ACP takes it as a thinking level.
    pub effort: String,
    pub prompt: String,
    /// The session to resume, for a follow-up.
    pub resume: Option<String>,
    /// Where an edit may go without asking: the worktree and the grant's
    /// directories, symlinks resolved.
    pub writable: Vec<PathBuf>,
    /// Explicitly shared read-only directories, symlinks resolved.
    pub read_only: Vec<PathBuf>,
}

const INITIALIZE: u64 = 1;
const SESSION: u64 = 2;
const SET_MODEL: u64 = 3;
const SET_THOUGHT: u64 = 4;
const SET_MODE: u64 = 5;
/// Prompts count up from here: the first, then one per steer.
const PROMPTS: u64 = 100;

/// What Perch sends agy when a command it left running in the background
/// ends. agy picks the result up by itself after its turn, but nothing
/// marks the end of a turn it starts unasked; a prompt's answer does, and
/// agy folds the two into one turn.
const COMMAND_ENDED: &str = "The command you left running has finished. Carry on from its result.";

/// `agy acp` over stdio. Perch opens or resumes one session, pins its
/// model, thinking level and mode, sends the prompt, and closes stdin
/// once the last prompt is answered, which ends the server and with it
/// any command agy still runs. So a turn agy ends with a command going
/// in the background leaves the session open until the command is done.
pub(crate) struct AcpClient {
    session: AcpSession,
    session_id: String,
    next_prompt: u64,
    /// Prompts sent and not yet answered.
    open_prompts: HashSet<u64>,
    /// The reader's messages not yet sent, and those sent (`(wire, raw)`)
    /// but not yet echoed back as read.
    queued: VecDeque<String>,
    sent: VecDeque<(String, String)>,
    /// Each steer's raw text by its request id.
    steer_ids: HashMap<u64, String>,
    /// A `session/prompt` result held while an OpenCode `_session/steering`
    /// call is still in flight, in case it replies `promptRequired`.
    pending_end: Option<Value>,
    /// The agent's text since its last step.
    text: String,
    /// Tool calls that have begun, and those that have ended.
    started: HashSet<String>,
    ended: HashSet<String>,
    /// agy's commands, but for daemons, which are meant to outlive the
    /// run; those running now; and those agy moved on from while they ran.
    /// A command that outlasts its wait goes on in the background, and agy
    /// may end the turn without it.
    commands: HashSet<String>,
    running: HashSet<String>,
    backgrounded: HashSet<String>,
    /// A command ended in the background and no prompt has gone since.
    unseen: bool,
    /// The last prompt is answered and a command is still running.
    held: bool,
    /// Each pending request by Perch's id: the JSON-RPC id to answer, and
    /// the options it offered.
    pending: HashMap<String, (Value, Vec<Value>)>,
    usage: Option<Value>,
    /// The reader refused a request and agy has said nothing since: agy
    /// ends its turn on a refusal, silently.
    refused: bool,
    done: bool,
}

impl AcpClient {
    pub fn new(session: AcpSession) -> Self {
        Self {
            session,
            session_id: String::new(),
            next_prompt: PROMPTS,
            open_prompts: HashSet::new(),
            queued: VecDeque::new(),
            sent: VecDeque::new(),
            steer_ids: HashMap::new(),
            pending_end: None,
            text: String::new(),
            started: HashSet::new(),
            ended: HashSet::new(),
            commands: HashSet::new(),
            running: HashSet::new(),
            backgrounded: HashSet::new(),
            unseen: false,
            held: false,
            pending: HashMap::new(),
            usage: None,
            refused: false,
            done: false,
        }
    }

    pub fn opencode_work(&self) -> bool {
        self.session.backend == crate::Backend::OpenCode && !self.session.writable.is_empty()
    }

    pub fn read_only_dirs(&self) -> &[PathBuf] {
        &self.session.read_only
    }

    pub fn opening(&mut self) -> Vec<String> {
        vec![request(
            INITIALIZE,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": {"readTextFile": false, "writeTextFile": false},
                    "terminal": false,
                },
                "clientInfo": {"name": "perch", "version": env!("CARGO_PKG_VERSION")},
            }),
        )]
    }

    /// agy steers a running turn with another prompt; OpenCode steers via
    /// `_session/steering`. One sent before a prompt is running waits for it.
    pub fn steer(&mut self, text: &str) -> Vec<String> {
        if self.done {
            return Vec::new();
        }
        if self.session_id.is_empty() || (self.open_prompts.is_empty() && !self.held) {
            self.queued.push_back(text.to_string());
            return Vec::new();
        }
        if self.session.backend == crate::Backend::OpenCode {
            let id = self.next_id();
            let wire = self.wire_prompt(text);
            self.steer_ids.insert(id, text.to_string());
            vec![request(
                id,
                "_session/steering",
                json!({
                    "sessionId": self.session_id,
                    "prompt": [{"type": "text", "text": wire}],
                }),
            )]
        } else {
            let (id, line) = self.prompt(text);
            self.sent
                .push_back((self.wire_prompt(text), text.to_string()));
            self.steer_ids.insert(id, text.to_string());
            vec![line]
        }
    }

    fn send_queued(&mut self) -> Reply {
        if self.queued.is_empty() {
            return Reply::default();
        }
        let text = self.queued.drain(..).collect::<Vec<_>>().join("\n\n");
        let (id, write) = self.prompt(&text);
        let mut events = self.flush_text();
        if self.session.backend == crate::Backend::OpenCode {
            events.push(RunEvent::Steered(text));
        } else {
            self.sent.push_back((self.wire_prompt(&text), text.clone()));
            self.steer_ids.insert(id, text);
        }
        Reply {
            writes: vec![write],
            events,
            ..Reply::default()
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_prompt;
        self.next_prompt += 1;
        id
    }

    fn wire_prompt(&self, text: &str) -> String {
        if self.session.backend == crate::Backend::OpenCode && text.trim_start().starts_with('/') {
            format!("Reader message:\n{text}")
        } else {
            text.to_string()
        }
    }

    fn prompt(&mut self, text: &str) -> (u64, String) {
        let id = self.next_id();
        self.open_prompts.insert(id);
        self.held = false;
        self.unseen = false;
        let wire = self.wire_prompt(text);
        (
            id,
            request(
                id,
                "session/prompt",
                json!({
                    "sessionId": self.session_id,
                    "prompt": [{"type": "text", "text": wire}],
                }),
            ),
        )
    }

    fn set_option(&self, id: u64, config: &str, value: &str) -> String {
        request(
            id,
            "session/set_config_option",
            json!({"sessionId": self.session_id, "configId": config, "value": value}),
        )
    }

    /// What the agent said since its last step, as one message.
    fn flush_text(&mut self) -> Vec<RunEvent> {
        let text = std::mem::take(&mut self.text);
        if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![RunEvent::Text(text.trim_end().to_string())]
        }
    }

    fn end(&mut self, event: RunEvent) -> Reply {
        self.done = true;
        let mut events = self.flush_text();
        if let Some(usage) = self.usage.take() {
            events.push(RunEvent::Usage {
                input_tokens: usage.get("inputTokens").and_then(Value::as_u64),
                output_tokens: usage.get("outputTokens").and_then(Value::as_u64),
                cached_input_tokens: usage.get("cachedReadTokens").and_then(Value::as_u64),
            });
        }
        events.push(event);
        Reply {
            events,
            close: true,
            ..Reply::default()
        }
    }

    fn fail(&mut self, message: String) -> Reply {
        self.end(RunEvent::Failed { message })
    }

    pub fn after_line(&mut self, v: &Value) -> Reply {
        if self.done {
            return Reply::default();
        }
        let method = v.get("method").and_then(Value::as_str);
        match (method, v.get("id")) {
            (None, Some(id)) => self.response(id.as_u64().unwrap_or(0), v),
            (Some(method), Some(id)) => self.server_request(id.clone(), method, v),
            (Some("session/update"), None) => {
                self.update(v.pointer("/params/update").unwrap_or(&Value::Null))
            }
            _ => Reply::default(),
        }
    }

    fn response(&mut self, id: u64, v: &Value) -> Reply {
        if self.session.backend == crate::Backend::OpenCode && self.steer_ids.contains_key(&id) {
            let raw_text = self.steer_ids.remove(&id).unwrap_or_default();
            if v.get("error").is_some() {
                let ready = self.open_prompts.is_empty() && self.steer_ids.is_empty();
                return match ready.then(|| self.pending_end.take()).flatten() {
                    Some(end) => self.turn_over(&end, Vec::new()),
                    None => Reply::default(),
                };
            }
            return match v.pointer("/result/outcome").and_then(Value::as_str) {
                Some("promptRequired") => {
                    self.queued.push_back(raw_text);
                    if self.open_prompts.is_empty() {
                        self.pending_end = None;
                        self.send_queued()
                    } else {
                        Reply::default()
                    }
                }
                // "injected", or any other acknowledgement of the steer.
                _ => {
                    let mut events = self.flush_text();
                    events.push(RunEvent::Steered(raw_text));
                    let ready = self.open_prompts.is_empty() && self.steer_ids.is_empty();
                    match ready.then(|| self.pending_end.take()).flatten() {
                        Some(end) => self.turn_over(&end, events),
                        None => Reply {
                            events,
                            ..Reply::default()
                        },
                    }
                }
            };
        }
        // A steer agy would not take is left unread, for Perch to send as
        // a follow-up, rather than failing the run it came into.
        if v.get("error").is_some() && self.steer_ids.contains_key(&id) {
            let text = self.steer_ids.remove(&id).unwrap_or_default();
            if let Some(at) = self.sent.iter().position(|(_, raw)| *raw == text) {
                self.sent.remove(at);
            }
            if !self.open_prompts.remove(&id) || !self.open_prompts.is_empty() {
                return Reply::default();
            }
            return self.turn_over(&Value::Null, Vec::new());
        }
        if let Some(error) = v.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("The agent refused the request")
                .to_string();
            return self.fail(one_line(&message, 300));
        }
        let result = v.get("result").cloned().unwrap_or(Value::Null);
        match id {
            INITIALIZE => {
                if self.session.backend == crate::Backend::OpenCode
                    && result
                        .pointer("/agentInfo/version")
                        .and_then(Value::as_str)
                        .and_then(crate::CliVersion::parse)
                        .is_none_or(|v| v < crate::MIN_OPENCODE_VERSION)
                {
                    return self.fail("Perch requires OpenCode 2.3.0 or newer".into());
                }
                let params = json!({
                    "cwd": self.session.cwd.to_string_lossy(),
                    "mcpServers": [],
                });
                let next = match &self.session.resume {
                    Some(id) => {
                        let mut params = params;
                        params["sessionId"] = json!(id);
                        request(SESSION, "session/resume", params)
                    }
                    None => request(SESSION, "session/new", params),
                };
                Reply {
                    writes: vec![next],
                    ..Reply::default()
                }
            }
            SESSION => {
                self.session_id = result
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| self.session.resume.clone())
                    .unwrap_or_default();
                Reply {
                    writes: vec![self.set_option(SET_MODEL, "model", &self.session.model)],
                    events: vec![RunEvent::Started {
                        session_id: self.session_id.clone(),
                        model: self.session.model.clone(),
                    }],
                    ..Reply::default()
                }
            }
            SET_MODEL if self.session.backend == crate::Backend::OpenCode => {
                let effort = result["configOptions"]
                    .as_array()
                    .and_then(|options| options.iter().find(|o| o["id"] == "effort"));
                let requested = self.session.effort.as_str();
                let write = match effort {
                    Some(option) => {
                        let offered = option["options"]
                            .as_array()
                            .is_some_and(|opts| opts.iter().any(|v| v["value"] == requested));
                        self.set_option(
                            SET_THOUGHT,
                            "effort",
                            if offered { requested } else { "default" },
                        )
                    }
                    None => self.set_option(
                        SET_MODE,
                        "mode",
                        crate::opencode::mode(self.opencode_work()),
                    ),
                };
                Reply {
                    writes: vec![write],
                    ..Reply::default()
                }
            }
            SET_THOUGHT if self.session.backend == crate::Backend::OpenCode => Reply {
                writes: vec![self.set_option(
                    SET_MODE,
                    "mode",
                    crate::opencode::mode(self.opencode_work()),
                )],
                ..Reply::default()
            },
            SET_MODEL => Reply {
                writes: vec![self.set_option(
                    SET_THOUGHT,
                    "thought_level",
                    thought_level(&self.session.effort),
                )],
                ..Reply::default()
            },
            SET_THOUGHT => Reply {
                writes: vec![request(
                    SET_MODE,
                    "session/set_mode",
                    json!({"sessionId": self.session_id, "modeId": "default"}),
                )],
                ..Reply::default()
            },
            SET_MODE => {
                let prompt = self.session.prompt.clone();
                let (_, first) = self.prompt(&prompt);
                let mut writes = vec![first];
                let mut events = Vec::new();
                if self.session.backend == crate::Backend::OpenCode {
                    let queued: Vec<String> = self.queued.drain(..).collect();
                    for text in queued {
                        writes.extend(self.steer(&text));
                    }
                } else {
                    let queued = self.send_queued();
                    writes.extend(queued.writes);
                    events.extend(queued.events);
                }
                Reply {
                    writes,
                    events,
                    ..Reply::default()
                }
            }
            id if self.open_prompts.remove(&id) => {
                let mut steered = Vec::new();
                if let Some(text) = self.steer_ids.remove(&id) {
                    if let Some(at) = self.sent.iter().position(|(_, raw)| *raw == text) {
                        self.sent.remove(at);
                        steered = self.flush_text();
                        steered.push(RunEvent::Steered(text));
                    }
                }
                if let Some(usage) = result.get("usage").filter(|u| u.is_object()) {
                    self.add_usage(usage);
                }
                if !self.open_prompts.is_empty() {
                    return Reply {
                        events: steered,
                        ..Reply::default()
                    };
                }
                self.turn_over(&result, steered)
            }
            _ => Reply::default(),
        }
    }

    /// The last open prompt is answered: send what came in meanwhile, or
    /// end the run.
    fn turn_over(&mut self, result: &Value, steered: Vec<RunEvent>) -> Reply {
        let mut reply = self.turn_end(result);
        let mut events = steered;
        events.append(&mut reply.events);
        reply.events = events;
        reply
    }

    fn turn_end(&mut self, result: &Value) -> Reply {
        // A steer that came as the turn ended goes as the next.
        if !self.queued.is_empty() {
            return self.send_queued();
        }
        if self.session.backend == crate::Backend::OpenCode {
            match result["stopReason"].as_str() {
                Some("max_tokens" | "max_turn_requests") => {
                    return self.fail("OpenCode reached its turn limit".into())
                }
                Some("cancelled") => return self.end(RunEvent::Stopped { forced: false }),
                _ => {}
            }
        }
        if result.get("stopReason").and_then(Value::as_str) == Some("refusal") {
            return self.fail("The model refused to go on".into());
        }
        if self.session.backend == crate::Backend::OpenCode && !self.steer_ids.is_empty() {
            self.pending_end = Some(result.clone());
            return Reply {
                events: self.flush_text(),
                ..Reply::default()
            };
        }
        // Closing now would kill the command; its end starts the next turn.
        if !self.running.is_empty() {
            self.backgrounded.extend(self.running.iter().cloned());
            self.held = true;
            return Reply {
                events: self.flush_text(),
                ..Reply::default()
            };
        }
        // One ended while the turn was going, too late for agy to take it in.
        if self.unseen {
            let events = self.flush_text();
            let (_, write) = self.prompt(COMMAND_ENDED);
            return Reply {
                writes: vec![write],
                events,
                ..Reply::default()
            };
        }
        let stopped = self.session.backend == crate::Backend::Gemini
            && self.refused
            && self.text.trim().is_empty();
        let session_id = self.session_id.clone();
        let mut reply = self.end(RunEvent::Finished {
            session_id,
            cost_usd: None,
            duration_ms: 0,
        });
        if stopped {
            let at = reply.events.len().saturating_sub(1);
            reply.events.insert(
                at,
                RunEvent::PermissionDenied {
                    tool: None,
                    message: "Gemini ends its turn when a request is refused. Reply to carry on."
                        .into(),
                },
            );
        }
        reply
    }

    fn update(&mut self, u: &Value) -> Reply {
        let events = match str_of(u, "sessionUpdate").as_str() {
            "agent_message_chunk" => {
                self.moved_on();
                if let Some(text) = u.pointer("/content/text").and_then(Value::as_str) {
                    self.text.push_str(text);
                    self.refused &= text.trim().is_empty();
                }
                Vec::new()
            }
            "agent_thought_chunk" => {
                self.moved_on();
                Vec::new()
            }
            "user_message_chunk" => {
                let text = u.pointer("/content/text").and_then(Value::as_str);
                match (text, self.sent.front()) {
                    (Some(text), Some((wire, _))) if text == wire => {
                        let mut events = self.flush_text();
                        if let Some((_, raw)) = self.sent.pop_front() {
                            events.push(RunEvent::Steered(raw));
                        }
                        events
                    }
                    _ => Vec::new(),
                }
            }
            "tool_call" | "tool_call_update" => {
                let events = self.tool(u);
                if self.held && self.unseen {
                    let (_, write) = self.prompt(COMMAND_ENDED);
                    return Reply {
                        writes: vec![write],
                        events,
                        ..Reply::default()
                    };
                }
                events
            }
            _ => Vec::new(),
        };
        Reply {
            events,
            ..Reply::default()
        }
    }

    /// agy thinks or speaks again only once the commands it ran have
    /// ended or gone to the background.
    fn moved_on(&mut self) {
        self.backgrounded.extend(self.running.iter().cloned());
    }

    /// agy counts each turn's tokens on its own.
    fn add_usage(&mut self, usage: &Value) {
        let Some(total) = self.usage.as_mut().and_then(Value::as_object_mut) else {
            self.usage = Some(usage.clone());
            return;
        };
        for (key, value) in usage.as_object().into_iter().flatten() {
            match (total.get(key).and_then(Value::as_u64), value.as_u64()) {
                (Some(a), Some(b)) => {
                    total.insert(key.clone(), json!(a + b));
                }
                _ => {
                    total.insert(key.clone(), value.clone());
                }
            }
        }
    }

    fn tool(&mut self, u: &Value) -> Vec<RunEvent> {
        let id = str_of(u, "toolCallId");
        if id.is_empty() || self.ended.contains(&id) {
            return Vec::new();
        }
        let mut events = self.flush_text();
        if self.started.insert(id.clone()) {
            self.refused = false;
            // Known by its first update; later ones may leave out the name.
            if self.session.backend == crate::Backend::Gemini
                && tool_name(u) == "run_command"
                && tool_args(u).get("IsDaemon").and_then(Value::as_bool) != Some(true)
            {
                self.commands.insert(id.clone());
            }
            events.push(RunEvent::ToolCall {
                id: id.clone(),
                name: tool_name(u),
                summary: tool_summary(u),
                parent_id: None,
            });
        }
        let status = str_of(u, "status");
        // A command waiting to be allowed is "pending", and a refused one
        // may never be heard of again. An update without a status changes
        // nothing.
        if self.commands.contains(&id) {
            match status.as_str() {
                "in_progress" => {
                    self.running.insert(id.clone());
                }
                "pending" => {
                    self.running.remove(&id);
                }
                "completed" | "failed" => {
                    self.running.remove(&id);
                    self.unseen |= self.backgrounded.remove(&id);
                }
                _ => {}
            }
        }
        if matches!(status.as_str(), "completed" | "failed") {
            self.ended.insert(id.clone());
            let exit = u
                .pointer("/rawOutput/exit_code")
                .or_else(|| u.pointer("/rawOutput/metadata/exit"))
                .and_then(Value::as_i64);
            let signal = u
                .pointer("/rawOutput/metadata/signal")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let killed = u
                .pointer("/rawOutput/metadata/killed")
                .and_then(Value::as_bool)
                == Some(true);
            let timed_out = u
                .pointer("/rawOutput/metadata/timeout")
                .and_then(Value::as_bool)
                == Some(true);
            let failed = status == "failed"
                || exit.is_some_and(|code| code != 0)
                || signal.is_some()
                || killed
                || timed_out;
            let summary = if failed {
                let text = u
                    .pointer("/rawOutput/error")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| {
                        u.pointer("/rawOutput/output")
                            .and_then(Value::as_str)
                            .filter(|s| !s.trim().is_empty())
                    })
                    .or_else(|| {
                        u.get("content")
                            .and_then(Value::as_array)
                            .and_then(|items| {
                                items.iter().rev().find_map(|item| {
                                    item.pointer("/content/text")
                                        .or_else(|| item.get("text"))
                                        .and_then(Value::as_str)
                                        .filter(|s| !s.trim().is_empty())
                                })
                            })
                    })
                    .unwrap_or("");
                let line = crate::codex::last_line(text);
                if !line.is_empty() {
                    line
                } else {
                    exit.filter(|&c| c != 0)
                        .map(|c| format!("Exit code {c}"))
                        .or_else(|| signal.map(|s| format!("Killed by {s}")))
                        .or_else(|| timed_out.then(|| "Timed out".to_string()))
                        .or_else(|| killed.then(|| "Killed".to_string()))
                        .unwrap_or_default()
                }
            } else {
                String::new()
            };
            events.push(RunEvent::ToolResult {
                id,
                ok: !failed,
                summary,
            });
        }
        events
    }

    fn server_request(&mut self, id: Value, method: &str, v: &Value) -> Reply {
        if method != "session/request_permission" {
            // Perch offers no file system or terminal of its own.
            return Reply {
                writes: vec![json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": "Perch does not handle this request"},
                })
                .to_string()],
                ..Reply::default()
            };
        }
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        let call = params.get("toolCall").cloned().unwrap_or(Value::Null);
        let options: Vec<Value> = params
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut events = self.flush_text();
        if self.session.backend == crate::Backend::OpenCode && !self.opencode_work() {
            return Reply {
                writes: answer_line(&id, &options, false).into_iter().collect(),
                events,
                ..Reply::default()
            };
        }
        if let Some(paths) = edit_paths(&call, &self.session.cwd) {
            if paths.as_ref().map_or(true, |paths| {
                paths.iter().any(|path| {
                    self.session
                        .read_only
                        .iter()
                        .any(|dir| path.starts_with(dir))
                        || is_protected(path, &self.session.cwd, &self.session.writable)
                })
            }) {
                self.refused = true;
                events.push(RunEvent::PermissionDenied {
                    tool: Some(tool_name(&call)).filter(|n| !n.is_empty()),
                    message: "Perch refused to edit a protected path".into(),
                });
                return Reply {
                    writes: answer_line(&id, &options, false).into_iter().collect(),
                    events,
                    ..Reply::default()
                };
            }
            if paths.is_ok_and(|paths| {
                !paths.is_empty()
                    && paths
                        .iter()
                        .all(|path| may_edit(path, &self.session.writable))
            }) {
                return Reply {
                    writes: answer_line(&id, &options, true).into_iter().collect(),
                    events,
                    ..Reply::default()
                };
            }
        }
        if self.session.backend == crate::Backend::Gemini
            && may_read(&call, &self.session.cwd, &self.session.read_only)
        {
            return Reply {
                writes: answer_line(&id, &options, true).into_iter().collect(),
                events,
                ..Reply::default()
            };
        }
        let key = match &id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        self.pending.insert(key.clone(), (id, options));
        let mut request = approval(&call, &self.session.cwd, &self.session.writable);
        if self.session.backend == crate::Backend::OpenCode {
            let input = tool_args(&call);
            match &request.kind {
                ApprovalKind::Command => {
                    if let Some(dir) = input["workdir"].as_str().map(|p| {
                        let joined = self.session.cwd.join(p);
                        resolve(&joined).unwrap_or(joined)
                    }) {
                        if dir != self.session.cwd {
                            // Include the directory in both the card and the exact command grant.
                            let quoted = dir.to_string_lossy().replace('\'', "'\\''");
                            request.subject = format!("cd '{quoted}' && {}", request.subject);
                        }
                    }
                }
                ApprovalKind::Tool { name } => {
                    let exact = if input.as_object().is_some_and(|m| !m.is_empty()) {
                        Some(input.to_string())
                    } else {
                        call.get("locations")
                            .and_then(Value::as_array)
                            .filter(|locs| !locs.is_empty())
                            .map(|locs| Value::Array(locs.clone()).to_string())
                    };
                    if request.subject == *name {
                        if let Some(locs) = call.get("locations").and_then(Value::as_array) {
                            let paths: Vec<String> = locs
                                .iter()
                                .map(|loc| str_of(loc, "path"))
                                .filter(|p| !p.is_empty())
                                .collect();
                            if !paths.is_empty() {
                                request.subject = paths.join(", ");
                            }
                        }
                    }
                    if let Some(exact) = exact {
                        // The display summary can be truncated; grant the exact arguments only.
                        request.suggestion = Some(exact.clone());
                        request.detail = Some(exact);
                    }
                }
                _ => {}
            }
        }
        events.push(RunEvent::Approval { id: key, request });
        Reply {
            events,
            ..Reply::default()
        }
    }

    pub fn answer(&mut self, id: &str, decision: &Decision) -> Option<String> {
        let (rpc_id, options) = self.pending.remove(id)?;
        let allow = matches!(decision, Decision::Allow);
        self.refused |= !allow;
        answer_line(&rpc_id, &options, allow)
    }
}

/// The reply that picks a request's one-time allow or reject, by the id
/// it offered for it, or cancels it when it offered none.
fn answer_line(id: &Value, options: &[Value], allow: bool) -> Option<String> {
    let kind = if allow { "allow_once" } else { "reject_once" };
    let outcome = options
        .iter()
        .find(|o| str_of(o, "kind") == kind)
        .map(|o| json!({"outcome": "selected", "optionId": str_of(o, "optionId")}))
        .unwrap_or_else(|| json!({"outcome": "cancelled"}));
    Some(json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": outcome}}).to_string())
}

fn tool_name(u: &Value) -> String {
    match str_of(u, "name") {
        n if n.is_empty() => str_of(u, "kind"),
        n => n,
    }
}

fn tool_args(u: &Value) -> Value {
    u.get("rawInput")
        .filter(|v| v.is_object())
        .or_else(|| u.get("args"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn tool_summary(u: &Value) -> String {
    let name = tool_name(u);
    let summary = summarize(&name, &tool_args(u));
    if summary == name {
        match str_of(u, "title") {
            t if t.trim().is_empty() => summary,
            t => one_line(&t, 120),
        }
    } else {
        summary
    }
}

const EDIT_PATH_KEYS: &[&str] = &[
    "Path",
    "TargetFile",
    "AbsolutePath",
    "File",
    "FilePath",
    "filePath",
    "path",
    "file_path",
    "target_file",
];

/// Auto-allow only named file reads and searches with explicit paths in
/// the granted directories. The CLI still asks about other tool calls.
fn may_read(call: &Value, cwd: &Path, dirs: &[PathBuf]) -> bool {
    if dirs.is_empty()
        || is_edit_call(call)
        || !matches!(
            tool_name(call).as_str(),
            "read"
                | "read_file"
                | "view_file"
                | "view_file_outline"
                | "view_code_item"
                | "list_dir"
                | "find_by_name"
                | "glob"
                | "grep"
                | "grep_search"
        )
    {
        return false;
    }
    let args = tool_args(call);
    let locations = call["locations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|location| location["path"].as_str());
    let arguments = EDIT_PATH_KEYS
        .iter()
        .copied()
        .chain([
            "DirectoryPath",
            "SearchPath",
            "SearchDirectory",
            "directory",
        ])
        .filter_map(|key| args[key].as_str());
    let mut paths = locations
        .chain(arguments)
        .filter(|path| !path.is_empty())
        .peekable();
    paths.peek().is_some()
        && paths.all(|path| {
            resolve_tool_path(cwd, path)
                .is_some_and(|real| dirs.iter().any(|dir| real.starts_with(dir)))
        })
}

fn is_edit_call(call: &Value) -> bool {
    str_of(call, "kind") == "edit"
        || matches!(
            tool_name(call).as_str(),
            "write"
                | "edit"
                | "patch"
                | "apply_patch"
                | "multiedit"
                | "write_file"
                | "write_to_file"
                | "replace_file_content"
                | "multi_replace_file_content"
                | "sed_file"
                | "edit_file"
                | "notebook_edit"
        )
}

/// The files an edit request writes, when it is one: ACP's `edit` kind,
/// with its locations, the paths in its input, or an `apply_patch` body,
/// each resolved against `cwd`.
fn edit_paths(call: &Value, cwd: &Path) -> Option<Result<Vec<PathBuf>, ()>> {
    if !is_edit_call(call) {
        return None;
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut unresolved = false;
    let mut push = |raw: &str| {
        if raw.is_empty() {
            return;
        }
        let Some(norm) = resolve_tool_path(cwd, raw) else {
            unresolved = true;
            return;
        };
        if !paths.contains(&norm) {
            paths.push(norm);
        }
    };
    if let Some(locations) = call.get("locations").and_then(Value::as_array) {
        for loc in locations {
            push(&str_of(loc, "path"));
        }
    }
    let args = tool_args(call);
    for key in EDIT_PATH_KEYS {
        if let Some(path) = args.get(*key).and_then(Value::as_str) {
            push(path);
        }
    }
    for list_key in ["files", "edits"] {
        if let Some(items) = args.get(list_key).and_then(Value::as_array) {
            for item in items {
                if let Some(path) = item.as_str() {
                    push(path);
                } else {
                    for key in EDIT_PATH_KEYS {
                        if let Some(path) = item.get(*key).and_then(Value::as_str) {
                            push(path);
                        }
                    }
                }
            }
        }
    }
    if let Some(patch) = args
        .get("patchText")
        .or_else(|| args.get("patch"))
        .and_then(Value::as_str)
    {
        for line in patch.lines() {
            let line = line.trim_end_matches('\r');
            for prefix in [
                "*** Add File: ",
                "*** Update File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ] {
                if let Some(rest) = line.strip_prefix(prefix) {
                    push(rest.trim());
                }
            }
        }
    }
    Some(if unresolved { Err(()) } else { Ok(paths) })
}

fn resolve_tool_path(cwd: &Path, raw: &str) -> Option<PathBuf> {
    let path = if raw == "~" || raw.starts_with("~/") {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        if !home.is_absolute() {
            return None;
        }
        home.join(raw[1..].trim_start_matches('/'))
    } else if raw.starts_with('~') {
        // A named user's home is ambiguous across tools.
        return None;
    } else {
        cwd.join(raw)
    };
    resolve(&path)
}

/// Resolve existing components through symlinks before appending any
/// missing file or directory names. Reject ambiguous paths.
fn resolve(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    match path.canonicalize() {
        Ok(real) => return Some(real),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return None,
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            other => {
                resolved.push(other);
                match resolved.symlink_metadata() {
                    Ok(meta) if meta.is_symlink() => resolved = resolved.canonicalize().ok()?,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return None,
                }
            }
        }
    }
    Some(resolved)
}

fn has_protected_name(path: &Path) -> bool {
    path.components().any(|part| {
        let part = part.as_os_str().to_string_lossy();
        crate::WORK_PROTECTED
            .iter()
            .any(|name| part.eq_ignore_ascii_case(name))
    })
}

/// Whether an edit to `path` may go without asking: under one of
/// `writable`, with no protected name anywhere below that root. Names
/// compare without case, as macOS's filesystem does, so `.Claude` is
/// `.claude`; and at any depth, so a grant of the worktree's parent
/// cannot reach the worktree's own `.claude`.
fn may_edit(path: &Path, writable: &[PathBuf]) -> bool {
    let Some(path) = resolve(path) else {
        return false;
    };
    writable.iter().any(|root| {
        let Ok(inside) = path.strip_prefix(root) else {
            return false;
        };
        inside.components().next().is_some() && !has_protected_name(inside)
    })
}

/// Whether `path` names a protected agent or git path, which a Work run
/// may not edit even under Allow all or a grant.
fn is_protected(path: &Path, cwd: &Path, writable: &[PathBuf]) -> bool {
    if may_edit(path, writable) {
        return false;
    }
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let Some(resolved) = resolve(&full) else {
        return has_protected_name(path);
    };
    let mut inside_any = false;
    for root in writable {
        if let Ok(inside) = resolved.strip_prefix(root) {
            inside_any = true;
            if has_protected_name(inside) {
                return true;
            }
        }
    }
    !inside_any && has_protected_name(&resolved)
}

/// A permission request as the card shows it.
fn approval(call: &Value, cwd: &Path, writable: &[PathBuf]) -> ApprovalRequest {
    let name = tool_name(call);
    let args = tool_args(call);
    let title = str_of(call, "title");
    match str_of(call, "kind").as_str() {
        "execute" => ApprovalRequest {
            kind: ApprovalKind::Command,
            subject: args
                .get("CommandLine")
                .or_else(|| args.get("command"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(title),
            detail: args
                .get("toolSummary")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(|s| one_line(s, 200)),
            suggestion: None,
        },
        kind if kind == "edit" || is_edit_call(call) => {
            // Every path that needs asking, so none hides behind one that
            // would have gone anyway, and "Always" keeps exactly that set.
            let asked: Vec<PathBuf> = edit_paths(call, cwd)
                .and_then(Result::ok)
                .unwrap_or_default()
                .into_iter()
                .filter(|p| !may_edit(p, writable))
                .collect();
            let subject = if asked.is_empty() {
                // No path given: the request as sent, so a grant for it
                // covers nothing broader.
                format!("{name} {}", one_line(&args.to_string(), 200))
            } else {
                asked
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let inside = !asked.is_empty()
                && asked.iter().all(|p| {
                    resolve(p).is_some_and(|p| writable.iter().any(|root| p.starts_with(root)))
                });
            ApprovalRequest {
                kind: ApprovalKind::Edit,
                subject,
                detail: inside.then(|| "A protected path in the worktree".to_string()),
                suggestion: None,
            }
        }
        _ => ApprovalRequest {
            subject: match summarize(&name, &args) {
                s if s == name && !title.trim().is_empty() => one_line(&title, 120),
                s => s,
            },
            kind: ApprovalKind::Tool { name },
            detail: None,
            suggestion: None,
        },
    }
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn print_mode_steps_become_events() {
        let mut p = GeminiParser::new();
        let started = p.parse_line(
            r#"{"event":"init","conversation_id":"c-1","init":{"model":"gemini-3.8-flash","cwd":"/wt","tools":[],"permission_mode":"proceed-in-sandbox"}}"#,
        );
        assert_eq!(
            started,
            vec![RunEvent::Started {
                session_id: "c-1".into(),
                model: "gemini-3.8-flash".into()
            }]
        );
        let call = p.parse_line(
            r#"{"event":"step_update","step_update":{"conversation_id":"c-1","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"git log -1"}}}}"#,
        );
        assert!(
            matches!(&call[..], [RunEvent::ToolCall { id, summary, .. }] if id == "step-2" && summary == "Ran git log -1"),
            "{call:?}"
        );
        let failed = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"git log -1"},"output":"Created At: x\nCommand exited with code 128.\n\nCommand output:\nfatal: bad revision\r\n"}}}"#,
        );
        assert_eq!(
            failed,
            vec![RunEvent::ToolResult {
                id: "step-2".into(),
                ok: false,
                summary: "fatal: bad revision".into()
            }]
        );
        let refused = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"tool","tool_name":"write_file","tool_info":{"name":"write_file","parameters":{"Path":"/wt/d.txt","Content":"y"},"error":{"type":"permission","message":"Permission denied for write_file(/wt/d.txt)"}}}}"#,
        );
        assert!(
            matches!(
                &refused[..],
                [RunEvent::ToolCall { summary, .. }, RunEvent::ToolResult { ok: false, summary: why, .. }]
                    if summary == "Edited wt/d.txt (+1 \u{2212}0)" && why.starts_with("Permission denied")
            ),
            "{refused:?}"
        );
        for delta in ["The answer", " is 4.", "\n"] {
            let partial = p.parse_line(&format!(
                r#"{{"event":"step_update","step_update":{{"step_index":4,"state":"ACTIVE","step_type":"agent_response","text_delta":{}}}}}"#,
                serde_json::to_string(delta).unwrap()
            ));
            assert!(partial.is_empty());
        }
        let said = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":4,"state":"DONE","step_type":"agent_response","text_delta":""}}"#,
        );
        assert_eq!(said, vec![RunEvent::Text("The answer is 4.".into())]);
        let done = p.parse_line(
            r#"{"event":"result","result":{"conversation_id":"c-1","status":"SUCCESS","response":"The answer is 4.","duration_seconds":2.5,"num_turns":1,"usage":{"input_tokens":10,"output_tokens":3,"cache_read_tokens":1}}}"#,
        );
        assert_eq!(
            done,
            vec![
                RunEvent::Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(3),
                    cached_input_tokens: Some(1)
                },
                RunEvent::Finished {
                    session_id: "c-1".into(),
                    cost_usd: None,
                    duration_ms: 2500
                }
            ]
        );
    }

    #[test]
    fn a_structured_result_is_said_and_an_error_fails_the_run() {
        let mut p = GeminiParser::new();
        let review = p.parse_line(
            r#"{"event":"result","result":{"conversation_id":"c","status":"SUCCESS","structured_output":{"status":"clean","findings":[]}}}"#,
        );
        assert!(
            matches!(&review[0], RunEvent::Text(t) if t.contains("\"clean\"")),
            "{review:?}"
        );
        let error = p.parse_line(
            r#"{"event":"result","result":{"conversation_id":"","status":"ERROR","error":"model x is not recognized"}}"#,
        );
        assert_eq!(
            error,
            vec![RunEvent::Failed {
                message: "model x is not recognized".into()
            }]
        );
    }

    #[test]
    fn the_print_host_holds_steers_for_the_next_turn() {
        let mut host = GeminiHost::new("Why?".into());
        let opening: Value = serde_json::from_str(&host.opening()[0]).unwrap();
        assert_eq!(opening["event"], "user");
        assert_eq!(opening["message"]["content"], "Why?");
        let prompt = host.after_line(&line(
            r#"{"event":"step_update","step_update":{"step_index":0,"state":"DONE","step_type":"user_input"}}"#,
        ));
        assert_eq!(prompt, Reply::default());
        assert!(
            host.steer("And how?").is_empty(),
            "held until the turn ends"
        );
        let turn = host.after_line(&line(r#"{"event":"result","result":{"status":"SUCCESS"}}"#));
        assert!(turn.more && !turn.close);
        let next: Value = serde_json::from_str(&turn.writes[0]).unwrap();
        assert_eq!(next["message"]["content"], "And how?");
        let read = host.after_line(&line(
            r#"{"event":"step_update","step_update":{"step_index":5,"state":"DONE","step_type":"user_input"}}"#,
        ));
        assert_eq!(read.events, vec![RunEvent::Steered("And how?".into())]);
        let end = host.after_line(&line(r#"{"event":"result","result":{"status":"SUCCESS"}}"#));
        assert!(end.close && !end.more);
    }

    #[test]
    fn the_print_host_does_not_steer_after_a_sandbox_failed_turn() {
        let sandbox_failed = r#"{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"git log -1"},"output":"Command failed to run: connecting to sandbox server: read unix @->@: recvmsg: connection reset by peer.\n\nNo output from the command."}}}"#;
        let empty = r#"{"event":"result","result":{"conversation_id":"c","status":"SUCCESS","response":""}}"#;
        // The parser fails this turn, so the host must not send the steer.
        let mut p = GeminiParser::new();
        p.parse_line(sandbox_failed);
        assert!(matches!(
            p.parse_line(empty).last(),
            Some(RunEvent::Failed { .. })
        ));
        let mut host = GeminiHost::new("Why?".into());
        host.opening();
        host.steer("And how?");
        assert_eq!(host.after_line(&line(sandbox_failed)), Reply::default());
        let turn = host.after_line(&line(empty));
        assert!(
            turn.close && !turn.more && turn.writes.is_empty(),
            "{turn:?}"
        );

        // A turn that answered anyway passes, and the failure belongs to
        // its turn: the next one, with no failed command, takes its steer.
        let mut host = GeminiHost::new("Why?".into());
        host.steer("And how?");
        host.steer("And when?");
        host.after_line(&line(sandbox_failed));
        let turn = host.after_line(&line(
            r#"{"event":"result","result":{"status":"SUCCESS","response":"Answered anyway."}}"#,
        ));
        assert!(
            turn.more && !turn.close && turn.writes.len() == 1,
            "{turn:?}"
        );
        let turn = host.after_line(&line(empty));
        assert!(
            turn.more && !turn.close && turn.writes.len() == 1,
            "{turn:?}"
        );
    }

    #[test]
    fn print_mode_surfaces_sandbox_launch_failures_and_error_states() {
        let mut p = GeminiParser::new();
        let started = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":3,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"git log -1"}}}}"#,
        );
        assert_eq!(started.len(), 1);
        let sandbox_failed = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"git log -1"},"output":"Created At: 2026-11-19T15:30:00-08:00\nCompleted At: 2026-11-19T15:30:02-08:00\nCommand failed to run: connecting to sandbox server: read unix @->@: recvmsg: connection reset by peer.\n\nNo output from the command."}}}"#,
        );
        assert!(
            matches!(
                &sandbox_failed[..],
                [RunEvent::ToolResult { id, ok: false, summary }]
                    if id == "step-3" && summary.starts_with("Command failed to run: connecting to sandbox server:")
            ),
            "{sandbox_failed:?}"
        );
        // A step that ends in ERROR emits a failed ToolResult with its error line.
        let _ = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":9,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"git log -1"}}}}"#,
        );
        let denied = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":9,"state":"ERROR","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"git log -1"},"error":{"type":"TOOL_ERROR","message":"permission check failed for unsandboxed \"git log -1\": user denied permission"}}}}"#,
        );
        assert!(
            matches!(
                &denied[..],
                [RunEvent::ToolResult { id, ok: false, summary }]
                    if id == "step-9" && summary.starts_with("permission check failed for unsandboxed")
            ),
            "{denied:?}"
        );
        // A command that exited 0 while echoing the failure phrase still counts as ok.
        let echoed = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":10,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo ok"},"output":"Created At: x\nCommand exited with code 0.\n\nCommand output:\nCommand failed to run: literal\n"}}}"#,
        );
        assert!(
            matches!(
                &echoed[..],
                [RunEvent::ToolCall { .. }, RunEvent::ToolResult { ok: true, summary, .. }]
                    if summary.is_empty()
            ),
            "{echoed:?}"
        );
        // An auto-denied command in print mode can finish DONE with empty output
        // and no error object; it must not be reported as a successful tool call.
        let _ = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":12,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hi > out.txt"}}}}"#,
        );
        let auto_denied = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":12,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hi > out.txt"}}}}"#,
        );
        assert_eq!(
            auto_denied,
            vec![RunEvent::ToolResult {
                id: "step-12".into(),
                ok: false,
                summary: "command did not run".into(),
            }]
        );
    }

    #[test]
    fn read_only_settings_deny_writes_and_the_web() {
        let dir = tempfile::tempdir().unwrap();
        let settings: Value = serde_json::from_str(&read_only_settings(dir.path(), &[])).unwrap();
        assert_eq!(settings["enableTerminalSandbox"], true);
        assert_eq!(settings["toolPermission"], "proceed-in-sandbox");
        let deny: Vec<&str> = settings["permissions"]["deny"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let real = crate::real_path(dir.path());
        assert!(deny.contains(&format!("write_file({})", real.display()).as_str()));
        for rule in ["read_url(*)", "execute_url(*)", "mcp(*)"] {
            assert!(deny.contains(&rule), "{rule}");
        }
        assert!(
            !deny.contains(&"unsandboxed(*)"),
            "agy 1.5.13 warns on and ignores unsandboxed(*): {deny:?}"
        );
        assert!(settings["permissions"]["allow"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn unsandboxed_read_only_settings_disable_sandbox_and_allow_read_only_commands() {
        let dir = tempfile::tempdir().unwrap();
        let settings: Value =
            serde_json::from_str(&unsandboxed_read_only_settings(dir.path(), &[])).unwrap();
        assert_eq!(settings["enableTerminalSandbox"], false);
        assert_eq!(settings["toolPermission"], "request-review");
        assert_eq!(settings["allowNonWorkspaceAccess"], false);
        let real = crate::real_path(dir.path());
        let allow: Vec<&str> = settings["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for rule in [
            "command(git diff)".to_string(),
            "command(git log)".to_string(),
            "command(git show)".to_string(),
            "command(git blame)".to_string(),
            format!("command(git -C {} diff)", real.display()),
            format!("command(git -C {} log)", real.display()),
            "command(git ls-files)".to_string(),
            "command(git grep)".to_string(),
            "command(git ls-tree)".to_string(),
            "command(git cat-file)".to_string(),
            "command(ls)".to_string(),
            "command(echo)".to_string(),
            "command(grep)".to_string(),
        ] {
            assert!(
                allow.contains(&rule.as_str()),
                "missing {rule} in {allow:?}"
            );
        }
        for forbidden in ["command(rg)", "command(find)", "command(sed)"] {
            assert!(
                !allow.contains(&forbidden),
                "should not allow {forbidden}: {allow:?}"
            );
        }
        let deny: Vec<&str> = settings["permissions"]["deny"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for rule in [
            "write_file(*)".to_string(),
            format!("write_file({})", real.display()),
            "read_url(*)".to_string(),
            "execute_url(*)".to_string(),
            "mcp(*)".to_string(),
            "command(git diff --output)".to_string(),
            format!("command(git -C {} log --output)", real.display()),
        ] {
            assert!(deny.contains(&rule.as_str()), "missing {rule} in {deny:?}");
        }
    }

    #[test]
    fn a_checkout_with_agy_config_that_runs_code_is_refused_unless_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        assert_eq!(refusal(&root, &[], false), None);
        for name in CONFIG_DIRS {
            let config = root.join(name);
            std::fs::create_dir(&config).unwrap();
            assert_eq!(refusal(&root, &[], false), None, "an empty {name}/");
            std::fs::write(config.join("hooks.json"), "{}").unwrap();
            let why = refusal(&root, &[], false).expect("refused");
            assert!(
                why.contains(&format!("{name}/hooks.json defines hooks")),
                "{why}"
            );
            assert!(why.contains("[repo.work] trust = true"), "{why}");
            assert_eq!(refusal(&root, &[], true), None);
            std::fs::remove_dir_all(&config).unwrap();
        }
        // One in a parent inside the repository counts.
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::create_dir(root.join(".agents")).unwrap();
        std::fs::write(root.join(".agents/mcp_config.json"), "{}").unwrap();
        let why = refusal(&sub, &[], false).expect("refused");
        assert!(why.contains("MCP servers"), "{why}");
    }

    /// slatedb's: a skill, its references, rules and a workflow, all text
    /// the model reads, which agy runs nothing from.
    #[test]
    fn skills_rules_and_markdown_workflows_are_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let skill = root.join(".agents/skills/simple-english");
        std::fs::create_dir_all(skill.join("references")).unwrap();
        std::fs::create_dir_all(skill.join("scripts")).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: simple-english\n---\n").unwrap();
        std::fs::write(skill.join("references/checklist.md"), "- short words").unwrap();
        std::fs::write(skill.join("scripts/check.py"), "print(1)").unwrap();
        std::fs::write(root.join(".agents/skills/loose.md"), "x").unwrap();
        std::fs::create_dir(root.join(".agents/rules")).unwrap();
        // agy resolves an include from the including file's directory.
        std::fs::write(root.join("CONTRIBUTING.md"), "be kind").unwrap();
        std::fs::write(
            root.join(".agents/rules/style.md"),
            "See @[the guide](../../CONTRIBUTING.md) and @[gone](/nowhere/at/all).",
        )
        .unwrap();
        std::fs::create_dir(root.join(".agents/workflows")).unwrap();
        std::fs::write(root.join(".agents/workflows/release.md"), "1. tag").unwrap();
        std::fs::write(root.join(".agents/README.md"), "what this is").unwrap();
        // What Finder and git leave behind.
        for junk in [
            ".agents/.DS_Store",
            ".agents/skills/.DS_Store",
            ".agents/rules/.gitkeep",
        ] {
            std::fs::write(root.join(junk), "").unwrap();
        }
        std::fs::write(root.join("CLAUDE.md"), "@[x](CONTRIBUTING.md)").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("CLAUDE.md", root.join("AGENTS.md")).unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/GEMINI.md"), "@[up](../CONTRIBUTING.md)").unwrap();
        assert_eq!(refusal(&root.join("sub"), &[], false), None);
    }

    #[test]
    fn anything_else_in_agy_config_is_refused() {
        let cases: &[(&str, &str)] = &[
            ("plugins/p/plugin.json", "adds plugins"),
            ("plugins.json", "adds plugins"),
            ("agents/a.md", "custom agents"),
            ("agents.json", "custom agents"),
            ("sidecars/s/sidecar.json", "sidecars"),
            ("mcp.json", "MCP servers"),
            ("skills.json", "can't vouch for"),
            ("config.json", "can't vouch for"),
            ("Skills/s/SKILL.md", "can't vouch for"),
            ("notes.txt", "can't vouch for"),
            ("workflows/flow/workflow.json", "can't vouch for"),
            ("rules/sub/r.md", "can't vouch for"),
            ("rules/r.txt", "can't vouch for"),
            ("skills/s/hooks.json", "inside a skill today"),
            ("skills/s/deep/MCP_CONFIG.JSON", "inside a skill today"),
            ("skills/s/.agents/x", "inside a skill today"),
            (
                "skills/s/.claude-plugin/plugin.json",
                "inside a skill today",
            ),
            ("skills/loose.txt", "can't vouch for"),
        ];
        for (file, says) in cases {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            std::fs::create_dir(root.join(".git")).unwrap();
            let path = root.join(".agents").join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "{}").unwrap();
            let why = refusal(&root, &[], false).unwrap_or_else(|| panic!("{file} accepted"));
            assert!(why.contains(says), "{file}: {why}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn links_in_agy_config_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::fs::create_dir_all(root.join(".agents/skills")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join(".agents/skills/s")).unwrap();
        let why = refusal(&root, &[], false).expect("refused");
        assert!(why.contains(".agents/skills/s is a symbolic link"), "{why}");
        std::fs::remove_dir_all(root.join(".agents")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join(".agent")).unwrap();
        assert!(refusal(&root, &[], false).is_some());
    }

    /// `personal_customization_dir` points agy at another directory's
    /// `.agents/`, whatever the config dirs here hold.
    #[test]
    fn a_gemini_config_json_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".gemini")).unwrap();
        std::fs::write(root.join(".gemini/settings.json"), "{}").unwrap();
        assert_eq!(refusal(&root, &[], false), None);
        std::fs::write(root.join(".gemini/config.json"), "{}").unwrap();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        let why = refusal(&sub, &[], false).expect("refused");
        assert!(why.contains(".gemini/config.json"), "{why}");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_from_outside_the_repository_in_the_prompt_is_refused() {
        let link = |target: &str, at: &Path| std::os::unix::fs::symlink(target, at).unwrap();
        let cases: &[(&str, &str, &str)] = &[
            (".agents/rules/r.md", "@[key]({secret})", "includes"),
            (
                ".agents/rules/r.md",
                "@[key](~/.aws/credentials)",
                "includes",
            ),
            (
                ".agents/rules/r.md",
                "@[up](../../../secret.txt)",
                "includes",
            ),
            (
                ".agents/workflows/w.md",
                "@[up](../../../secret.txt)",
                "includes",
            ),
            (
                "AGENTS.md",
                "Read @[this](file:///etc/hosts) first.",
                "includes",
            ),
            ("GEMINI.md", "@[a](ok.md) then @[b]({secret})", "includes"),
            ("AGENTS.md", "\u{FFFD}@[b](../secret.txt)", "includes"),
            ("AGENTS.md", "@[s](docs/link.txt)", "includes"),
            ("AGENTS.md", "LINK", "links to a file outside"),
        ];
        for (file, text, says) in cases {
            let dir = tempfile::tempdir().unwrap();
            let tmp = dir.path().canonicalize().unwrap();
            let secret = tmp.join("secret.txt");
            std::fs::write(&secret, "hunter2").unwrap();
            let root = tmp.join("repo");
            std::fs::create_dir_all(root.join(".git")).unwrap();
            std::fs::create_dir_all(root.join("docs")).unwrap();
            link(secret.to_str().unwrap(), &root.join("docs/link.txt"));
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            if *text == "LINK" {
                link("../secret.txt", &path);
            } else {
                let text = text.replace("{secret}", secret.to_str().unwrap());
                // A byte that isn't UTF-8 ahead of the include.
                let mut bytes = text.replace('\u{FFFD}', "").into_bytes();
                if text.starts_with('\u{FFFD}') {
                    bytes.insert(0, 0xFF);
                }
                std::fs::write(&path, bytes).unwrap();
            }
            let why = refusal(&root, &[], false).unwrap_or_else(|| panic!("{file}: {text}"));
            assert!(why.contains(says), "{file}: {why}");
            assert!(why.contains("outside the repository"), "{why}");
        }
        assert_eq!(includes("@[a](b) @[c] (d) @[e](f"), ["b"]);
    }

    #[cfg(unix)]
    #[test]
    fn config_perch_cannot_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let config = root.join(".agents");
        std::fs::create_dir(&config).unwrap();
        std::fs::write(config.join("hooks.json"), "{}").unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o311)).unwrap();
        let why = refusal(&root, &[], false);
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).unwrap();
        let why = why.expect("refused");
        assert!(why.contains("can't read .agents"), "{why}");
    }

    #[test]
    fn a_config_too_large_to_check_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let rules = root.join(".agents/rules");
        std::fs::create_dir_all(&rules).unwrap();
        for n in 0..=SCAN_LIMIT {
            std::fs::write(rules.join(format!("{n}.md")), "").unwrap();
        }
        let why = refusal(&root, &[], false).expect("refused");
        assert!(why.contains("too large"), "{why}");
    }

    /// agy takes every `--add-dir` as a workspace and walks it up to its
    /// own repository's top: a linked worktree's git directories lead to
    /// the main checkout's configuration. Above a directory with no
    /// repository it reads nothing.
    #[test]
    fn added_dirs_bring_their_own_repositories_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("main");
        let gitdir = main.join(".git/worktrees/wt");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..").unwrap();
        let wt = root.join("wt");
        std::fs::create_dir(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        std::fs::create_dir(main.join(".agents")).unwrap();
        std::fs::write(main.join(".agents/hooks.json"), "{}").unwrap();

        assert_eq!(refusal(&wt, &[], false), None, "Work adds nothing");
        let why = refusal(&wt, &print_dirs(&wt, &[]), false).expect("refused");
        assert!(
            why.contains("main/.agents/hooks.json defines hooks"),
            "{why}"
        );
        assert!(why.contains("Work reads only this checkout"), "{why}");
        assert!(!why.contains("trust = true"), "{why}");

        // A discussion directory outside any repository counts alone.
        let context = root.join("context/review-1");
        std::fs::create_dir_all(&context).unwrap();
        std::fs::create_dir(root.join("context/.agents")).unwrap();
        std::fs::write(root.join("context/.agents/hooks.json"), "{}").unwrap();
        let clean = root.join("clean");
        std::fs::create_dir_all(clean.join(".git")).unwrap();
        assert_eq!(refusal(&clean, std::slice::from_ref(&context), false), None);
        std::fs::create_dir(context.join(".agents")).unwrap();
        std::fs::write(context.join(".agents/hooks.json"), "{}").unwrap();
        assert!(refusal(&clean, &[context], false).is_some());
    }

    fn session(dir: &Path) -> AcpSession {
        AcpSession {
            cwd: dir.to_path_buf(),
            backend: crate::Backend::Gemini,
            model: "gemini-4-carbon".into(),
            effort: "max".into(),
            prompt: "Fix it".into(),
            resume: None,
            writable: vec![crate::real_path(dir)],
            read_only: Vec::new(),
        }
    }

    fn written(reply: &Reply) -> Value {
        serde_json::from_str(&reply.writes[0]).unwrap()
    }

    #[test]
    fn acp_opens_a_session_pins_it_and_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        let init: Value = serde_json::from_str(&acp.opening()[0]).unwrap();
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["params"]["clientCapabilities"]["terminal"], false);
        let new = written(&acp.after_line(&line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)));
        assert_eq!(new["method"], "session/new");
        assert_eq!(new["params"]["mcpServers"], json!([]));
        let opened = acp.after_line(&line(r#"{"id":2,"result":{"sessionId":"s-1"}}"#));
        assert_eq!(
            opened.events,
            vec![RunEvent::Started {
                session_id: "s-1".into(),
                model: "gemini-4-carbon".into()
            }]
        );
        let model = written(&opened);
        assert_eq!(model["method"], "session/set_config_option");
        assert_eq!(model["params"]["configId"], "model");
        assert_eq!(model["params"]["value"], "gemini-4-carbon");
        let thought = written(&acp.after_line(&line(r#"{"id":3,"result":{}}"#)));
        assert_eq!(thought["params"]["configId"], "thought_level");
        assert_eq!(thought["params"]["value"], "high");
        let mode = written(&acp.after_line(&line(r#"{"id":4,"result":{}}"#)));
        assert_eq!(mode["method"], "session/set_mode");
        assert_eq!(mode["params"]["modeId"], "default");
        let prompt = written(&acp.after_line(&line(r#"{"id":5,"result":{}}"#)));
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["prompt"][0]["text"], "Fix it");

        let chunk = acp.after_line(&line(
            r#"{"method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Looking."}}}}"#,
        ));
        assert!(chunk.events.is_empty());
        let call = acp.after_line(&line(
            r#"{"method":"session/update","params":{"update":{"sessionUpdate":"tool_call","toolCallId":"t1","name":"run_command","kind":"execute","status":"in_progress","title":"cargo test","rawInput":{"CommandLine":"cargo test"}}}}"#,
        ));
        assert_eq!(call.events[0], RunEvent::Text("Looking.".into()));
        assert!(
            matches!(&call.events[1], RunEvent::ToolCall { id, summary, .. } if id == "t1" && summary == "Ran cargo test")
        );
        let ran = acp.after_line(&line(
            r#"{"method":"session/update","params":{"update":{"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","rawOutput":{"output":"error: 1 failed\n","exit_code":101}}}}"#,
        ));
        assert_eq!(
            ran.events,
            vec![RunEvent::ToolResult {
                id: "t1".into(),
                ok: false,
                summary: "error: 1 failed".into()
            }]
        );
        let end = acp.after_line(&line(
            r#"{"id":100,"result":{"stopReason":"end_turn","usage":{"inputTokens":5,"outputTokens":2,"cachedReadTokens":1}}}"#,
        ));
        assert!(end.close);
        assert!(
            matches!(
                &end.events[..],
                [RunEvent::Usage { input_tokens: Some(5), .. }, RunEvent::Finished { session_id, .. }] if session_id == "s-1"
            ),
            "{:?}",
            end.events
        );
    }

    #[test]
    fn acp_resumes_a_session_without_replaying_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(AcpSession {
            resume: Some("s-0".into()),
            ..session(dir.path())
        });
        acp.opening();
        let resume = written(&acp.after_line(&line(r#"{"id":1,"result":{}}"#)));
        assert_eq!(resume["method"], "session/resume");
        assert_eq!(resume["params"]["sessionId"], "s-0");
        let opened = acp.after_line(&line(r#"{"id":2,"result":{}}"#));
        assert!(
            matches!(&opened.events[0], RunEvent::Started { session_id, .. } if session_id == "s-0")
        );
    }

    #[test]
    fn acp_approves_archive_reads_and_searches_and_refuses_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let archive = root.join("archive");
        let file = archive.join("probe.md");
        std::fs::create_dir(&archive).unwrap();
        std::fs::write(&file, "marker\n").unwrap();
        let mut acp = AcpClient::new(AcpSession {
            read_only: vec![archive.clone()],
            ..session(&root)
        });
        let permission = |id, name, kind, input| {
            json!({
                "id": id,
                "method": "session/request_permission",
                "params": {
                    "toolCall": {"name": name, "kind": kind, "rawInput": input},
                    "options": [
                        {"optionId": "allow", "kind": "allow_once"},
                        {"optionId": "deny", "kind": "reject_once"}
                    ]
                }
            })
        };
        for call in [
            permission("read", "read_file", "read", json!({"Path": file})),
            permission(
                "search",
                "grep_search",
                "search",
                json!({"SearchPath": archive}),
            ),
        ] {
            let reply = acp.after_line(&call);
            assert!(reply.events.is_empty());
            assert_eq!(written(&reply)["result"]["outcome"]["optionId"], "allow");
        }
        let home = PathBuf::from(std::env::var_os("HOME").unwrap())
            .canonicalize()
            .unwrap();
        let relative_to_home = "../".repeat(home.components().count() - 1);
        let tilde = |path: &Path| {
            format!(
                "~/{relative_to_home}{}",
                path.strip_prefix("/").unwrap().display()
            )
        };
        let missing_file = archive.join("missing/checks.md");
        let mut read = permission(
            "missing",
            "read_file",
            "read",
            json!({"Path": tilde(&missing_file)}),
        );
        read["params"]["toolCall"]["locations"] = json!([{"path": missing_file}]);
        let reply = acp.after_line(&read);
        assert!(reply.events.is_empty());
        assert_eq!(written(&reply)["result"]["outcome"]["optionId"], "allow");

        let edit = acp.after_line(&permission(
            "edit",
            "write_file",
            "edit",
            json!({"Path": tilde(&missing_file)}),
        ));
        assert!(matches!(
            edit.events.as_slice(),
            [RunEvent::PermissionDenied { .. }]
        ));
        assert_eq!(written(&edit)["result"]["outcome"]["optionId"], "deny");
        let outside = acp.after_line(&permission(
            "outside",
            "grep_search",
            "search",
            json!({"SearchPath": root}),
        ));
        assert!(outside.writes.is_empty());
        assert!(matches!(
            outside.events.as_slice(),
            [RunEvent::Approval { .. }]
        ));
    }

    #[test]
    fn acp_edits_in_the_worktree_go_and_the_rest_come_to_the_reader() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::real_path(dir.path());
        let mut acp = AcpClient::new(session(dir.path()));
        let ask = |path: &str, id: u64| {
            format!(
                r#"{{"jsonrpc":"2.0","method":"session/request_permission","id":"{id}","params":{{"sessionId":"s","toolCall":{{"toolCallId":"w{id}","name":"write_file","kind":"edit","title":"File","locations":[{{"path":"{path}"}}],"rawInput":{{"Path":"{path}","Content":"x"}}}},"options":[{{"optionId":"allow","name":"Allow","kind":"allow_once"}},{{"optionId":"deny","name":"Deny","kind":"reject_once"}}]}}}}"#
            )
        };
        let inside = acp.after_line(&line(&ask(&root.join("src/new.rs").to_string_lossy(), 1)));
        assert!(inside.events.is_empty(), "{:?}", inside.events);
        let allowed = written(&inside);
        assert_eq!(allowed["id"], "1");
        assert_eq!(allowed["result"]["outcome"]["optionId"], "allow");

        for (n, path) in [
            root.join(".agents/hooks.json"),
            root.join("_agents/x"),
            root.join("_agent/x"),
            root.join("sub/.agents/hooks.json"),
            root.join(".git/hooks/pre-commit"),
            root.join(".claude/settings.json"),
            PathBuf::from(".agents/hooks.json"),
            PathBuf::from("/tmp/elsewhere/.git/hooks/pre-commit"),
        ]
        .iter()
        .enumerate()
        {
            let id = n as u64 + 10;
            let refused = acp.after_line(&line(&ask(&path.to_string_lossy(), id)));
            let reply = written(&refused);
            assert_eq!(
                reply["result"]["outcome"]["optionId"], "deny",
                "{path:?} was not refused outright"
            );
            assert!(
                matches!(
                    &refused.events[..],
                    [RunEvent::PermissionDenied {
                        tool: Some(tool),
                        ..
                    }] if tool == "write_file"
                ),
                "{path:?}: {:?}",
                refused.events
            );
        }

        for (n, path) in [
            root.join("src/../../escape.txt"),
            PathBuf::from("/etc/hosts"),
        ]
        .iter()
        .enumerate()
        {
            let id = n as u64 + 30;
            let asked = acp.after_line(&line(&ask(&path.to_string_lossy(), id)));
            assert!(asked.writes.is_empty(), "{path:?} went without asking");
            assert!(
                matches!(
                    &asked.events[..],
                    [RunEvent::Approval {
                        request: ApprovalRequest {
                            kind: ApprovalKind::Edit,
                            ..
                        },
                        ..
                    }]
                ),
                "{path:?}: {:?}",
                asked.events
            );
        }
        let command = acp.after_line(&line(
            r#"{"jsonrpc":"2.0","method":"session/request_permission","id":"c1","params":{"toolCall":{"toolCallId":"t","name":"run_command","kind":"execute","title":"curl x","rawInput":{"CommandLine":"curl https://example.com","toolSummary":"Fetch the page"}},"options":[{"optionId":"allow","kind":"allow_once"},{"optionId":"deny","kind":"reject_once"}]}}"#,
        ));
        assert_eq!(
            command.events,
            vec![RunEvent::Approval {
                id: "c1".into(),
                request: ApprovalRequest {
                    kind: ApprovalKind::Command,
                    subject: "curl https://example.com".into(),
                    detail: Some("Fetch the page".into()),
                    suggestion: None,
                }
            }]
        );
        let denied: Value =
            serde_json::from_str(&acp.answer("c1", &Decision::deny("no")).unwrap()).unwrap();
        assert_eq!(denied["id"], "c1");
        assert_eq!(denied["result"]["outcome"]["optionId"], "deny");
        assert_eq!(acp.answer("c1", &Decision::Allow), None, "answered once");

        let other = acp.after_line(&line(
            r#"{"jsonrpc":"2.0","method":"fs/read_text_file","id":9,"params":{}}"#,
        ));
        assert!(other.writes[0].contains("\"error\""));
    }

    #[test]
    fn a_linked_worktree_s_git_dirs_are_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "one",
            ],
        );
        let wt = root.join("wt");
        git(&repo, &["worktree", "add", "-q", wt.to_str().unwrap()]);

        let args = print_args(&wt, &[], true, "m", "low");
        let added: Vec<&String> = args
            .windows(2)
            .filter(|w| w[0] == "--add-dir")
            .map(|w| &w[1])
            .collect();
        let git_dir = repo.join(".git").to_string_lossy().into_owned();
        assert!(added.contains(&&git_dir), "{added:?}");
        let settings = &args[args.iter().position(|a| a == "--settings").unwrap() + 1];
        assert!(
            settings.contains(&format!("write_file({git_dir})")),
            "{settings}"
        );
    }

    #[test]
    fn borrowed_objects_are_read_only() {
        // A review snapshot's checkout keeps its history in a sibling
        // repo.git through alternates, chained here one further.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let far = root.join("far.git/objects");
        let near = root.join("repo.git/objects");
        let own = root.join("checkout/.git/objects");
        for dir in [&far, &near, &own] {
            std::fs::create_dir_all(dir.join("info")).unwrap();
        }
        std::fs::write(near.join("info/alternates"), format!("{}\n", far.display())).unwrap();
        std::fs::write(
            own.join("info/alternates"),
            "# borrowed\n../../../repo.git/objects\n",
        )
        .unwrap();

        let args = print_args(&root.join("checkout"), &[], true, "m", "low");
        let added: Vec<&String> = args
            .windows(2)
            .filter(|w| w[0] == "--add-dir")
            .map(|w| &w[1])
            .collect();
        let near = near.to_string_lossy().into_owned();
        let far = far.to_string_lossy().into_owned();
        assert_eq!(added, [&near, &far]);
        let settings = &args[args.iter().position(|a| a == "--settings").unwrap() + 1];
        assert!(
            settings.contains(&format!("write_file({near})")),
            "{settings}"
        );
    }

    #[test]
    fn the_api_schema_has_no_null_in_an_enum() {
        let schema = api_schema(&crate::review_output_schema());
        assert!(
            !schema.contains(",null]") && !schema.contains("[null"),
            "{schema}"
        );
        let value: Value = serde_json::from_str(&schema).unwrap();
        assert_eq!(
            value.pointer("/properties/findings/items/properties/side/type"),
            Some(&json!(["string", "null"]))
        );
    }

    #[test]
    fn protected_names_ask_in_any_case_at_any_depth_under_any_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().canonicalize().unwrap();
        let wt = parent.join("wt");
        std::fs::create_dir(&wt).unwrap();
        let writable = vec![wt.clone(), parent.clone()];
        assert!(may_edit(&wt.join("src/lib.rs"), &writable));
        assert!(may_edit(&parent.join("notes.txt"), &writable));
        for path in [
            wt.join(".Claude/settings.json"),
            wt.join(".AGENTS/hooks.json"),
            wt.join("_AGENT/hooks.json"),
            wt.join(".MCP.json"),
            wt.join("sub/.GIT/config"),
            wt.join("packages/app/.claude/settings.json"),
            wt.join(".claude/settings.json"),
        ] {
            assert!(!may_edit(&path, &writable), "{path:?}");
            assert!(is_protected(&path, &wt, &writable), "{path:?}");
        }
        // A worktree inside `.claude/worktrees/` still allows its own code
        // while keeping `.claude` outside it protected.
        let nested_wt = parent.join(".claude/worktrees/feat");
        std::fs::create_dir_all(&nested_wt).unwrap();
        let nested_writable = vec![nested_wt.clone()];
        assert!(may_edit(&nested_wt.join("src/lib.rs"), &nested_writable));
        assert!(!is_protected(
            &nested_wt.join("src/lib.rs"),
            &nested_wt,
            &nested_writable
        ));
        assert!(is_protected(
            &parent.join(".claude/settings.json"),
            &nested_wt,
            &nested_writable
        ));
    }

    #[test]
    fn an_edit_card_names_every_path_that_asks() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().canonicalize().unwrap();
        let writable = vec![wt.clone()];
        let call = json!({
            "kind": "edit",
            "name": "write_file",
            "locations": [
                {"path": wt.join("README.md")},
                {"path": "/etc/hosts"},
                {"path": wt.join(".claude/settings.json")},
            ],
        });
        let request = approval(&call, &wt, &writable);
        assert_eq!(
            request.subject,
            format!("/etc/hosts, {}", wt.join(".claude/settings.json").display())
        );
        assert_eq!(request.detail, None, "not all of it is in the worktree");
        let bare = approval(
            &json!({"kind": "edit", "name": "write_file", "rawInput": {"x": 1}}),
            &wt,
            &writable,
        );
        assert_ne!(bare.subject, "files");
    }

    #[test]
    fn a_refusal_that_ends_the_turn_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        acp.opening();
        for id in 1..=5 {
            acp.after_line(&line(&format!(
                r#"{{"id":{id},"result":{{"sessionId":"s"}}}}"#
            )));
        }
        acp.after_line(&line(
            r#"{"jsonrpc":"2.0","method":"session/request_permission","id":"c1","params":{"toolCall":{"toolCallId":"t","name":"run_command","kind":"execute","rawInput":{"CommandLine":"rm x"}},"options":[{"optionId":"allow","kind":"allow_once"},{"optionId":"deny","kind":"reject_once"}]}}"#,
        ));
        acp.answer("c1", &Decision::deny("no"));
        let end = acp.after_line(&line(r#"{"id":100,"result":{"stopReason":"end_turn"}}"#));
        assert!(
            matches!(
                &end.events[..],
                [
                    RunEvent::PermissionDenied { tool: None, .. },
                    RunEvent::Finished { .. }
                ]
            ),
            "{:?}",
            end.events
        );
    }

    #[test]
    fn what_print_mode_refused_is_shown() {
        // When an auto-denied tool aborts a headless turn without producing
        // any answer, agy still prints status=SUCCESS and response=""; Perch
        // must fail the run rather than report it as completed.
        let mut parser = GeminiParser::new();
        let events = parser.parse_line(
            r#"{"event":"result","result":{"conversation_id":"c","status":"SUCCESS","response":"","denied_actions":[{"action":"read_file","display_name":"ReadFile"}]}}"#,
        );
        assert_eq!(
            events[0],
            RunEvent::PermissionDenied {
                tool: Some("ReadFile".into()),
                message: "agy did not allow it".into(),
            }
        );
        assert!(
            matches!(
                events.last(),
                Some(RunEvent::Failed { message }) if message.contains("ReadFile")
            ),
            "{events:?}"
        );

        // If agy recovered and still produced a response, the run finishes
        // while keeping the denied-action notice in the transcript.
        let mut recovered = GeminiParser::new();
        let events = recovered.parse_line(
            r#"{"event":"result","result":{"conversation_id":"c","status":"SUCCESS","response":"Done anyway.","denied_actions":[{"action":"read_file","display_name":"ReadFile"}]}}"#,
        );
        assert!(matches!(events[0], RunEvent::PermissionDenied { .. }));
        assert!(matches!(events.last(), Some(RunEvent::Finished { .. })));

        // When a sandbox connection failure preceded the empty denied result,
        // the failure message points at Gemini's per-agent sandbox setting.
        let mut sandbox_fail = GeminiParser::new();
        let _ = sandbox_fail.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"git log -1"},"output":"Command failed to run: connecting to sandbox server: read unix @->@: recvmsg: connection reset by peer.\n\nNo output from the command."}}}"#,
        );
        let events = sandbox_fail.parse_line(
            r#"{"event":"result","result":{"conversation_id":"c","status":"SUCCESS","response":"","denied_actions":[{"action":"command","display_name":"Bash"}]}}"#,
        );
        assert!(
            matches!(
                events.last(),
                Some(RunEvent::Failed { message })
                    if message.contains("sandbox") && message.contains("[agent.gemini] sandbox = false")
            ),
            "{events:?}"
        );
    }

    #[test]
    fn acp_steers_go_as_prompts_and_land_when_echoed() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        acp.opening();
        for id in 1..=4 {
            acp.after_line(&line(&format!(
                r#"{{"id":{id},"result":{{"sessionId":"s"}}}}"#
            )));
        }
        assert!(acp.steer("Early").is_empty(), "no prompt going yet");
        let started = acp.after_line(&line(r#"{"id":5,"result":{}}"#));
        assert_eq!(started.writes.len(), 2, "the prompt, then the early steer");
        let steer = acp.steer("And the docs");
        let steer: Value = serde_json::from_str(&steer[0]).unwrap();
        assert_eq!(steer["method"], "session/prompt");
        let echoed = acp.after_line(&line(
            r#"{"method":"session/update","params":{"update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"Early"}}}}"#,
        ));
        assert_eq!(echoed.events, vec![RunEvent::Steered("Early".into())]);
        let first = acp.after_line(&line(r#"{"id":100,"result":{"stopReason":"end_turn"}}"#));
        assert!(!first.close, "two prompts still open");
        let echoed_early =
            acp.after_line(&line(r#"{"id":101,"result":{"stopReason":"end_turn"}}"#));
        assert!(echoed_early.events.is_empty(), "Early landed once");
        // Answered without an echo: read all the same.
        let last = acp.after_line(&line(r#"{"id":102,"result":{"stopReason":"end_turn"}}"#));
        assert!(last.close);
        assert_eq!(last.events[0], RunEvent::Steered("And the docs".into()));
    }

    #[test]
    fn a_steer_agy_will_not_take_is_left_unread() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        acp.opening();
        for id in 1..=5 {
            acp.after_line(&line(&format!(
                r#"{{"id":{id},"result":{{"sessionId":"s"}}}}"#
            )));
        }
        acp.steer("Late");
        let refused = acp.after_line(&line(
            r#"{"id":101,"error":{"code":-32603,"message":"busy"}}"#,
        ));
        assert!(!refused.close && refused.events.is_empty());
        let end = acp.after_line(&line(r#"{"id":100,"result":{"stopReason":"end_turn"}}"#));
        assert!(end.close);
        assert!(!end
            .events
            .iter()
            .any(|e| matches!(e, RunEvent::Steered(_) | RunEvent::Failed { .. })));
    }

    fn command(id: &str, status: &str) -> Value {
        json!({"method": "session/update", "params": {"update": {
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "name": "run_command",
            "kind": "execute",
            "status": status,
            "rawInput": {"CommandLine": "cargo test", "WaitMsBeforeAsync": 500},
            "rawOutput": {"output": "ok\n", "exit_code": 0},
        }}})
    }

    #[test]
    fn a_turn_that_leaves_a_command_running_waits_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        acp.opening();
        for id in 1..=5 {
            acp.after_line(&line(&format!(
                r#"{{"id":{id},"result":{{"sessionId":"s"}}}}"#
            )));
        }
        // As agy sends them: in progress, pending while it asks, then running.
        for status in ["in_progress", "pending", "in_progress"] {
            acp.after_line(&command("t1", status));
        }
        acp.after_line(&line(
            r#"{"method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I will continue once it finishes."}}}}"#,
        ));
        let held = acp.after_line(&line(r#"{"id":100,"result":{"stopReason":"end_turn"}}"#));
        assert!(!held.close && held.writes.is_empty());
        assert_eq!(
            held.events,
            vec![RunEvent::Text("I will continue once it finishes.".into())]
        );
        // A message meanwhile goes straight in, and its turn's end waits too.
        let steer: Value = serde_json::from_str(&acp.steer("Any news?")[0]).unwrap();
        assert_eq!(steer["id"], 101);
        let still = acp.after_line(&line(r#"{"id":101,"result":{"stopReason":"end_turn"}}"#));
        assert!(!still.close && still.writes.is_empty());
        assert_eq!(still.events, vec![RunEvent::Steered("Any news?".into())]);
        // The command's end starts a turn whose answer ends the run.
        let ended = acp.after_line(&command("t1", "completed"));
        assert!(!ended.close);
        assert!(matches!(
            ended.events.last(),
            Some(RunEvent::ToolResult { ok: true, .. })
        ));
        let nudge = written(&ended);
        assert_eq!(nudge["method"], "session/prompt");
        assert_eq!(nudge["id"], 102);
        assert_eq!(nudge["params"]["prompt"][0]["text"], COMMAND_ENDED);
        let echoed = acp.after_line(&json!({"method": "session/update", "params": {"update": {
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": COMMAND_ENDED},
        }}}));
        assert!(
            echoed.events.is_empty(),
            "Perch's own words are not the reader's"
        );
        let done = acp.after_line(&line(r#"{"id":102,"result":{"stopReason":"end_turn"}}"#));
        assert!(done.close);
        assert!(matches!(
            done.events.last(),
            Some(RunEvent::Finished { .. })
        ));
    }

    #[test]
    fn a_command_that_ends_in_its_turn_or_is_never_allowed_holds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        for statuses in [
            &["in_progress", "completed"][..],
            &["in_progress", "pending"],
        ] {
            let mut acp = AcpClient::new(session(dir.path()));
            acp.opening();
            for id in 1..=5 {
                acp.after_line(&line(&format!(
                    r#"{{"id":{id},"result":{{"sessionId":"s"}}}}"#
                )));
            }
            for status in statuses {
                let reply = acp.after_line(&command("t1", status));
                assert!(reply.writes.is_empty());
            }
            let end = acp.after_line(&line(r#"{"id":100,"result":{"stopReason":"end_turn"}}"#));
            assert!(end.close, "{statuses:?}");
        }
    }

    fn ready(dir: &Path) -> AcpClient {
        let mut acp = AcpClient::new(session(dir));
        acp.opening();
        for id in 1..=5 {
            acp.after_line(&line(&format!(
                r#"{{"id":{id},"result":{{"sessionId":"s"}}}}"#
            )));
        }
        acp
    }

    fn thought() -> Value {
        json!({"method": "session/update", "params": {"update": {
            "sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "Waiting on it."},
        }}})
    }

    fn turn_end(id: u64) -> Value {
        json!({"id": id, "result": {"stopReason": "end_turn"}})
    }

    #[test]
    fn a_command_that_ends_in_the_background_before_the_turn_does_is_followed_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = ready(dir.path());
        acp.after_line(&command("t1", "in_progress"));
        // agy goes on while it runs, so it went to the background.
        acp.after_line(&thought());
        let ended = acp.after_line(&command("t1", "completed"));
        assert!(ended.writes.is_empty(), "the turn is still going");
        let end = acp.after_line(&turn_end(100));
        assert!(!end.close);
        let nudge = written(&end);
        assert_eq!(nudge["id"], 101);
        assert_eq!(nudge["params"]["prompt"][0]["text"], COMMAND_ENDED);
        assert!(acp.after_line(&turn_end(101)).close);
    }

    #[test]
    fn each_background_command_s_end_is_taken_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = ready(dir.path());
        acp.after_line(&command("t1", "in_progress"));
        acp.after_line(&command("t2", "in_progress"));
        assert!(acp.after_line(&turn_end(100)).writes.is_empty());
        let first = acp.after_line(&command("t1", "completed"));
        assert_eq!(written(&first)["id"], 101);
        // t2 ends while agy answers for t1.
        assert!(acp
            .after_line(&command("t2", "completed"))
            .writes
            .is_empty());
        let next = acp.after_line(&turn_end(101));
        assert!(!next.close);
        assert_eq!(written(&next)["id"], 102);
        assert!(acp.after_line(&turn_end(102)).close);
    }

    #[test]
    fn a_steer_s_turn_still_takes_in_a_command_that_ends_during_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = ready(dir.path());
        acp.after_line(&command("t1", "in_progress"));
        acp.after_line(&turn_end(100));
        acp.steer("Any news?");
        acp.after_line(&command("t1", "completed"));
        let end = acp.after_line(&turn_end(101));
        assert!(!end.close);
        assert_eq!(written(&end)["params"]["prompt"][0]["text"], COMMAND_ENDED);
    }

    #[test]
    fn an_update_without_a_name_or_status_leaves_a_command_running() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = ready(dir.path());
        acp.after_line(&command("t1", "in_progress"));
        acp.after_line(&json!({"method": "session/update", "params": {"update": {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "t1",
            "content": [{"type": "content", "content": {"type": "text", "text": "Compiling"}}],
        }}}));
        let end = acp.after_line(&turn_end(100));
        assert!(!end.close && end.writes.is_empty());
        let ended = acp.after_line(&json!({"method": "session/update", "params": {"update": {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "t1",
            "status": "completed",
        }}}));
        assert_eq!(
            written(&ended)["params"]["prompt"][0]["text"],
            COMMAND_ENDED
        );
    }

    #[test]
    fn a_daemon_does_not_hold_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = ready(dir.path());
        acp.after_line(&json!({"method": "session/update", "params": {"update": {
            "sessionUpdate": "tool_call",
            "toolCallId": "d1",
            "name": "run_command",
            "kind": "execute",
            "status": "in_progress",
            "rawInput": {"CommandLine": "npm run dev", "IsDaemon": true},
        }}}));
        acp.after_line(&thought());
        assert!(acp.after_line(&turn_end(100)).close);
    }

    #[test]
    fn usage_adds_up_across_turns() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = ready(dir.path());
        acp.after_line(&command("t1", "in_progress"));
        acp.after_line(&json!({"id": 100, "result": {"stopReason": "end_turn",
            "usage": {"inputTokens": 10, "outputTokens": 2, "cachedReadTokens": 5}}}));
        acp.after_line(&command("t1", "completed"));
        let done = acp.after_line(&json!({"id": 101, "result": {"stopReason": "end_turn",
            "usage": {"inputTokens": 4, "outputTokens": 1, "cachedReadTokens": 7}}}));
        assert!(done.events.contains(&RunEvent::Usage {
            input_tokens: Some(14),
            output_tokens: Some(3),
            cached_input_tokens: Some(12),
        }));
    }

    #[test]
    fn acp_errors_end_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        acp.opening();
        let failed = acp.after_line(&line(
            r#"{"id":1,"error":{"code":-32603,"message":"Failed to start Antigravity"}}"#,
        ));
        assert!(failed.close);
        assert_eq!(
            failed.events,
            vec![RunEvent::Failed {
                message: "Failed to start Antigravity".into()
            }]
        );
        assert_eq!(
            acp.after_line(&line(r#"{"id":2,"result":{}}"#)),
            Reply::default()
        );
    }

    #[test]
    fn summaries_read_agy_s_own_argument_names() {
        assert_eq!(
            summarize("view_file", &json!({"AbsolutePath": "/a/b/c/d.rs"})),
            "Read b/c/d.rs"
        );
        assert_eq!(
            summarize(
                "grep_search",
                &json!({"Query": "fn main", "SearchPath": "/wt/src"})
            ),
            "Searched for fn main in wt/src"
        );
        assert_eq!(summarize("finish", &json!({})), "finish");
    }

    fn opencode_ready(cwd: &Path, writable: Vec<PathBuf>) -> AcpClient {
        let mut acp = AcpClient::new(AcpSession {
            backend: crate::Backend::OpenCode,
            cwd: cwd.to_path_buf(),
            model: "provider/model".into(),
            effort: "high".into(),
            prompt: "Fix it".into(),
            resume: None,
            writable,
            read_only: Vec::new(),
        });
        acp.opening();
        acp.after_line(&json!({"id": 1, "result": {"agentInfo": {"version": "2.3.0"}}}));
        acp.after_line(&json!({"id": 2, "result": {"sessionId": "oc-1"}}));
        acp.after_line(&json!({
            "id": 3,
            "result": {"configOptions": [{"id": "effort", "options": [{"value": "high"}]}]},
        }));
        acp.after_line(&json!({"id": 4, "result": {}}));
        acp.after_line(&json!({"id": 5, "result": {}}));
        acp
    }

    #[test]
    fn opencode_tool_failures_and_nonzero_exits_report_their_reason() {
        let mut acp = opencode_ready(Path::new("/repo"), vec![PathBuf::from("/repo")]);
        let shell = acp.after_line(&json!({
            "method": "session/update",
            "params": {"update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": "s1",
                "name": "shell",
                "kind": "execute",
                "status": "completed",
                "rawInput": {"command": "cargo test"},
                "rawOutput": {"metadata": {"exit": 1}},
                "content": [{"type": "content", "content": {"type": "text", "text": "running 1 test\nerror: test failed\n"}}],
            }},
        }));
        assert_eq!(
            shell.events.last(),
            Some(&RunEvent::ToolResult {
                id: "s1".into(),
                ok: false,
                summary: "error: test failed".into(),
            })
        );

        let silent_exit = acp.after_line(&json!({
            "method": "session/update",
            "params": {"update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": "s2",
                "name": "shell",
                "kind": "execute",
                "status": "completed",
                "rawOutput": {"metadata": {"exit": 2}},
                "content": [],
            }},
        }));
        assert_eq!(
            silent_exit.events.last(),
            Some(&RunEvent::ToolResult {
                id: "s2".into(),
                ok: false,
                summary: "Exit code 2".into(),
            })
        );

        let killed = acp.after_line(&json!({
            "method": "session/update",
            "params": {"update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": "s3",
                "name": "shell",
                "kind": "execute",
                "status": "completed",
                "rawOutput": {"metadata": {"signal": "SIGKILL", "killed": true}},
            }},
        }));
        assert_eq!(
            killed.events.last(),
            Some(&RunEvent::ToolResult {
                id: "s3".into(),
                ok: false,
                summary: "Killed by SIGKILL".into(),
            })
        );

        let failed = acp.after_line(&json!({
            "method": "session/update",
            "params": {"update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": "s4",
                "name": "read",
                "kind": "read",
                "status": "failed",
                "rawOutput": {"error": "Permission denied: read"},
            }},
        }));
        assert_eq!(
            failed.events.last(),
            Some(&RunEvent::ToolResult {
                id: "s4".into(),
                ok: false,
                summary: "Permission denied: read".into(),
            })
        );
    }

    #[test]
    fn opencode_steers_use_session_steering_and_queue_on_prompt_required() {
        let mut acp = opencode_ready(Path::new("/repo"), vec![PathBuf::from("/repo")]);
        let writes = acp.steer("/review src/lib.rs");
        assert_eq!(writes.len(), 1);
        let steer: Value = serde_json::from_str(&writes[0]).unwrap();
        assert_eq!(steer["id"], 101);
        assert_eq!(steer["method"], "_session/steering");
        assert_eq!(steer["params"]["sessionId"], "oc-1");
        assert_eq!(
            steer["params"]["prompt"][0]["text"],
            "Reader message:\n/review src/lib.rs"
        );

        let injected = acp.after_line(&json!({"id": 101, "result": {"outcome": "injected"}}));
        assert!(!injected.close && injected.writes.is_empty());
        assert_eq!(
            injected.events,
            vec![RunEvent::Steered("/review src/lib.rs".into())]
        );

        let late = acp.steer("/explain");
        let late_msg: Value = serde_json::from_str(&late[0]).unwrap();
        assert_eq!(late_msg["id"], 102);
        assert_eq!(late_msg["method"], "_session/steering");
        let required = acp.after_line(&json!({
            "id": 102,
            "result": {"outcome": "promptRequired", "reason": "noRunningTurn"},
        }));
        assert!(!required.close && required.writes.is_empty() && required.events.is_empty());

        let next = acp.after_line(&json!({"id": 100, "result": {"stopReason": "end_turn"}}));
        assert!(!next.close);
        assert_eq!(next.events, vec![RunEvent::Steered("/explain".into())]);
        let follow: Value = serde_json::from_str(&next.writes[0]).unwrap();
        assert_eq!(follow["id"], 103);
        assert_eq!(follow["method"], "session/prompt");
        assert_eq!(
            follow["params"]["prompt"][0]["text"],
            "Reader message:\n/explain"
        );

        let done = acp.after_line(&json!({"id": 103, "result": {"stopReason": "end_turn"}}));
        assert!(done.close);
        assert!(
            matches!(done.events.last(), Some(RunEvent::Finished { session_id, .. }) if session_id == "oc-1")
        );

        // When stdout delivers end_turn before _session/steering's promptRequired,
        // the client holds completion until the steer response starts the next turn.
        let mut raced = opencode_ready(Path::new("/repo"), vec![PathBuf::from("/repo")]);
        raced.steer("One more thing");
        let held = raced.after_line(&json!({"id": 100, "result": {"stopReason": "end_turn"}}));
        assert!(!held.close && held.writes.is_empty());
        let resumed = raced.after_line(&json!({
            "id": 101,
            "result": {"outcome": "promptRequired", "reason": "noRunningTurn"},
        }));
        assert_eq!(
            resumed.events,
            vec![RunEvent::Steered("One more thing".into())]
        );
        let resumed_req: Value = serde_json::from_str(&resumed.writes[0]).unwrap();
        assert_eq!(resumed_req["id"], 102);
        assert_eq!(resumed_req["method"], "session/prompt");
        assert!(
            raced
                .after_line(&json!({"id": 102, "result": {"stopReason": "end_turn"}}))
                .close
        );
    }

    #[test]
    fn opencode_edit_permissions_normalize_and_deduplicate_relative_paths() {
        let mut acp = opencode_ready(Path::new("/wt/repo"), vec![PathBuf::from("/wt/repo")]);
        let dedup = acp.after_line(&json!({
            "jsonrpc": "2.0",
            "method": "session/request_permission",
            "id": "p1",
            "params": {
                "toolCall": {
                    "toolCallId": "w1",
                    "name": "write",
                    "kind": "edit",
                    "locations": [{"path": "/outside/secret.txt"}],
                    "rawInput": {"path": "../../outside/secret.txt"},
                },
                "options": [
                    {"optionId": "allow", "kind": "allow_once"},
                    {"optionId": "deny", "kind": "reject_once"},
                ],
            },
        }));
        assert_eq!(
            dedup.events,
            vec![RunEvent::Approval {
                id: "p1".into(),
                request: ApprovalRequest {
                    kind: ApprovalKind::Edit,
                    subject: "/outside/secret.txt".into(),
                    detail: None,
                    suggestion: None,
                },
            }]
        );

        let mixed = acp.after_line(&json!({
            "jsonrpc": "2.0",
            "method": "session/request_permission",
            "id": "p2",
            "params": {
                "toolCall": {
                    "toolCallId": "w2",
                    "name": "patch",
                    "kind": "edit",
                    "rawInput": {
                        "patchText": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** Add File: ../../outside/secret.txt\n+secret\n*** End Patch"
                    },
                },
                "options": [
                    {"optionId": "allow", "kind": "allow_once"},
                    {"optionId": "deny", "kind": "reject_once"},
                ],
            },
        }));
        assert_eq!(
            mixed.events,
            vec![RunEvent::Approval {
                id: "p2".into(),
                request: ApprovalRequest {
                    kind: ApprovalKind::Edit,
                    subject: "/outside/secret.txt".into(),
                    detail: None,
                    suggestion: None,
                },
            }]
        );

        let inside = acp.after_line(&json!({
            "jsonrpc": "2.0",
            "method": "session/request_permission",
            "id": "p3",
            "params": {
                "toolCall": {
                    "toolCallId": "w3",
                    "name": "edit",
                    "kind": "edit",
                    "rawInput": {"filePath": "src/lib.rs"},
                },
                "options": [
                    {"optionId": "allow", "kind": "allow_once"},
                    {"optionId": "deny", "kind": "reject_once"},
                ],
            },
        }));
        assert!(inside.events.is_empty(), "{:?}", inside.events);
        assert_eq!(written(&inside)["result"]["outcome"]["optionId"], "allow");
    }
}
