//! Run a coding agent as a child process and read what it does.
//!
//! Two agents: Claude Code, driven as `claude -p`, and Codex, driven as
//! `codex app-server` or, for a review, `codex exec`, each with JSON
//! events on stdout. No SDK and no library:
//! the binary on `PATH`, a child process, one JSON object per line. Perch
//! works with whatever version of the CLI the user has, and the parsers
//! are the whole contract with them.

mod codex;
mod discover;
mod interactive;
mod parse;
mod review;
pub mod usage;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[cfg(unix)]
use std::os::fd::{FromRawFd, OwnedFd};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;
use tokio::sync::Notify;

pub use codex::CodexParser;
pub use discover::{
    codex_catalog, codex_default_model, find_binary, install_dirs, parse_codex_catalog,
    path_with_install_dirs, CodexModel,
};
pub use interactive::{ApprovalKind, ApprovalRequest, Decision};
use interactive::{ClaudeHost, CodexServer, CodexThread, Conversation};
pub use parse::{parse_line, summarize_tool_call};
pub use review::{
    assess_review_capability, detect_review_capability, extract_review, preserve_native_response,
    review_output_schema, CliVersion, EvidenceSpan, ReviewCapability, ReviewEffort,
    ReviewExtraction, ReviewFinding, ReviewLocation, ReviewManifest, ReviewManifestEntry,
    ReviewSeverity, ReviewSide, ReviewSpec, ReviewVerificationSpec, REPORT_DELIMITER,
    REVIEW_ADAPTER_VERSION, REVIEW_OUTPUT_FORMAT_VERSION,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    ClaudeCode,
    Codex,
}

impl Backend {
    pub const ALL: [Backend; 2] = [Backend::ClaudeCode, Backend::Codex];

    pub fn name(self) -> &'static str {
        match self {
            Backend::ClaudeCode => "claude-code",
            Backend::Codex => "codex",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|b| b.name().eq_ignore_ascii_case(s.trim()))
    }
}

/// How hard the agent thinks. The top rung, `Ultra`, also lets it
/// delegate: Claude Code gets its orchestration tools, Codex its helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effort {
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
    Ultra,
}

impl Effort {
    pub const ALL: [Effort; 6] = [
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::XHigh,
        Effort::Max,
        Effort::Ultra,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
            Effort::Ultra => "ultra",
        }
    }

    /// For the screen: "Medium", "XHigh".
    pub fn label(self) -> &'static str {
        match self {
            Effort::Low => "Low",
            Effort::Medium => "Medium",
            Effort::High => "High",
            Effort::XHigh => "XHigh",
            Effort::Max => "Max",
            Effort::Ultra => "Ultra",
        }
    }

    /// What `claude --effort` calls it. Ultra is not a level of its own
    /// there: "ultracode" is xhigh plus the orchestration tools.
    fn claude_name(self) -> &'static str {
        match self {
            Effort::Ultra => "ultracode",
            other => other.name(),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|e| e.name().eq_ignore_ascii_case(s.trim()))
    }
}

impl std::fmt::Display for Effort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The tools a read-only run may use. In print mode anything not on the
/// list is denied without asking, and the denial arrives as an event. The
/// shell commands run inside Claude Code's sandbox as well, see
/// [`claude_settings`]. `rg` is not here: `rg --pre` runs any program.
pub const READ_ONLY_TOOLS: &[&str] = &[
    "Read",
    "Glob",
    "Grep",
    "LS",
    "Bash(git diff:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git blame:*)",
];

/// Tools that change files, named so that no setting of the user's can let
/// them through.
const WRITING_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Tools that reach the network from inside Claude Code itself, where the
/// shell sandbox does not apply. Denied by name for the same reason.
const NETWORK_TOOLS: &[&str] = &["WebFetch", "WebSearch"];

/// Everything a run is denied, whatever the user's settings allow.
pub fn denied_tools() -> impl Iterator<Item = String> {
    WRITING_TOOLS
        .iter()
        .chain(NETWORK_TOOLS)
        .map(|tool| (*tool).to_string())
}

/// Claude Code's delegation tools, allowed only at `Effort::Ultra`. The init
/// event lists the subagent tool as `Task` and the model calls it `Agent`,
/// so both are named. Subagents inherit the parent's allowed and denied
/// lists, which keeps them read-only too.
pub const ORCHESTRATION_TOOLS: &[&str] = &["Workflow", "Agent", "Task"];

/// The `--settings` every Claude Code run starts with. They outrank the
/// user's file, and the repository's files are not read at all.
///
/// The shell runs in Claude Code's own sandbox: no network, and no writes
/// anywhere, the worktree included, so `git diff --output=` fails like any
/// other write. Commands still go through the allowed list; the sandbox
/// is the second boundary, not a licence to run more. There is no
/// unsandboxed retry, and a machine without the sandbox refuses to run
/// rather than running without it.
pub fn claude_settings(cwd: &std::path::Path) -> String {
    claude_settings_with_dirs(cwd, &[])
}

/// [`claude_settings`] for a run also given `dirs` with `--add-dir`. That
/// opens them to the shell's writes as well as its reads, so they are
/// denied writes like the worktree.
pub fn claude_settings_with_dirs(cwd: &std::path::Path, dirs: &[PathBuf]) -> String {
    let deny_write: Vec<PathBuf> = std::iter::once(cwd)
        .chain(dirs.iter().map(PathBuf::as_path))
        .map(real_path)
        .collect();
    serde_json::json!({
        "sandbox": {
            "enabled": true,
            "autoAllowBashIfSandboxed": false,
            "allowUnsandboxedCommands": false,
            "failIfUnavailable": true,
            "filesystem": {
                "denyWrite": deny_write,
            },
            "network": {
                "allowedDomains": [],
                "deniedDomains": ["*"],
                "strictAllowlist": true,
            },
        },
    })
    .to_string()
}

/// What a run may do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Mode {
    /// Read the worktree and answer: [`READ_ONLY_TOOLS`], and a shell
    /// that writes nowhere.
    #[default]
    Ask,
    /// Edit the worktree and run commands in it, in a sandbox that
    /// writes only to the worktree, the temporary directory and the
    /// grant, and reaches only the grant's domains.
    Work(WorkGrant),
}

/// What a Work run may reach beyond its worktree: `[repo.work]`, and
/// what the reader has granted the repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkGrant {
    pub write: Vec<PathBuf>,
    pub network: Vec<String>,
    /// The repository's git directories are writable, all but their
    /// config and hooks, so the run can commit.
    pub commit: bool,
    /// The repository's own agent configuration is loaded.
    pub trust: bool,
    /// The CLI's own automatic reviewer decides what the sandbox refuses:
    /// Claude Code's auto permission mode, Codex's `auto_review`. What it
    /// passes on, or all of it when this is off, comes to Perch.
    pub auto_review: bool,
}

/// The tools a Work run may use without asking, besides editing inside
/// the worktree (see [`work_edit_rules`]). `Bash` is not among them: a
/// sandboxed command runs through `autoAllowBashIfSandboxed`, and one
/// asking to leave the sandbox then comes to Perch. Listed, `Bash`
/// would let it leave without asking.
pub const WORK_TOOLS: &[&str] = &["Read", "Glob", "Grep", "LS", "TodoWrite"];

/// The tools that edit files, allowed to a Work run only under its
/// worktree. `NotebookEdit` is among them; for Ask it is denied by name.
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// What in a worktree a Work run may not write: the file that points at
/// the git directory, and the agents' own configuration, which a later
/// run, or the reader's own agent, would take as instructions.
pub const WORK_PROTECTED: &[&str] = &[".git", ".claude", ".codex", ".mcp.json"];

/// Permission rules scoping the editing tools to the worktree, in Claude
/// Code's absolute-path form (`//` before the path), with the protected
/// paths in it denied, and a `.git` at any depth. `(allowed, denied)`.
pub fn work_edit_rules(cwd: &std::path::Path) -> (Vec<String>, Vec<String>) {
    let real = real_path(cwd);
    let root = real.to_string_lossy();
    let root = root.trim_end_matches('/');
    let allowed = EDIT_TOOLS
        .iter()
        .map(|tool| format!("{tool}(/{root}/**)"))
        .collect();
    let denied = EDIT_TOOLS
        .iter()
        .flat_map(|tool| {
            WORK_PROTECTED
                .iter()
                .flat_map(move |name| {
                    [
                        format!("{tool}(/{root}/{name})"),
                        format!("{tool}(/{root}/{name}/**)"),
                    ]
                })
                .chain([
                    format!("{tool}(/{root}/**/.git)"),
                    format!("{tool}(/{root}/**/.git/**)"),
                ])
        })
        .collect();
    (allowed, denied)
}

/// A `.git` anywhere below the worktree's top, as a sandbox glob. Git
/// takes a directory holding one, at a path the index has as a gitlink,
/// for a submodule, and a `git status` or `git commit` outside the
/// sandbox then runs git in there under that `.git`'s config, which the
/// run wrote. The `*/` keeps the pattern off the top-level `.git`, which
/// the commit grant opens when it is the repository's git directory:
/// Claude Code's `**/` also matches no directory at all.
fn nested_git(worktree: &std::path::Path) -> PathBuf {
    worktree.join("*/**/.git")
}

/// The git directories a linked worktree uses, which live outside it:
/// its own under `worktrees/<name>`, which its `.git` file names, and the
/// repository's, which that one's `commondir` names. Empty for a
/// checkout whose `.git` is a directory inside it.
pub fn linked_git_dirs(worktree: &std::path::Path) -> Vec<PathBuf> {
    let Ok(pointer) = std::fs::read_to_string(worktree.join(".git")) else {
        return Vec::new();
    };
    let Some(gitdir) = pointer.trim().strip_prefix("gitdir:") else {
        return Vec::new();
    };
    let gitdir = worktree.join(gitdir.trim());
    let mut dirs = vec![real_path(&gitdir)];
    if let Ok(common) = std::fs::read_to_string(gitdir.join("commondir")) {
        dirs.push(real_path(&gitdir.join(common.trim())));
    }
    dirs
}

/// What in a git directory steers git rather than holding the history,
/// which a run that may commit still may not write.
const GIT_CONTROL: [&str; 4] = ["config", "config.worktree", "commondir", "hooks"];

/// The git directories a commit in the worktree writes: a linked
/// worktree's two (see [`linked_git_dirs`]), or its own `.git`, the
/// repository's last.
fn git_dirs(worktree: &std::path::Path) -> Vec<PathBuf> {
    match linked_git_dirs(worktree) {
        dirs if !dirs.is_empty() => dirs,
        _ => vec![worktree.join(".git")],
    }
}

/// Where a commit writes, and what in there must stay read-only, for a
/// worktree whose repository the reader lets Work commit in: the
/// worktree's git directories, and each one's config and hooks, and its
/// `commondir`, which git follows to the config and hooks it uses: a
/// rewritten one makes the next `git commit` outside the sandbox run
/// hooks the run wrote.
///
/// The repository's git directory also holds what git uses elsewhere,
/// which stays read-only too: every other worktree's directory under
/// `worktrees/`, whose `commondir` and `config.worktree` would steer git
/// in that worktree, and `modules/`, the submodules' git directories.
/// A deny outranks an allow, so the run's own directory there can't be
/// carved out of a denied `worktrees/`; the others are named one by one,
/// and the globs cover a worktree added while the run is going.
fn commit_paths(worktree: &std::path::Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let dirs = git_dirs(worktree);
    let mut guarded: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|dir| GIT_CONTROL.map(|name| dir.join(name)))
        .collect();
    let common = dirs.last().expect("one git directory at least");
    let worktrees = common.join("worktrees");
    guarded.extend(["commondir", "config.worktree"].map(|name| worktrees.join("*").join(name)));
    if let Ok(entries) = std::fs::read_dir(&worktrees) {
        let mut others: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| real_path(&entry.path()))
            .filter(|dir| !dirs.contains(dir))
            .collect();
        others.sort();
        guarded.extend(others);
    }
    guarded.push(common.join("modules"));
    (dirs, guarded)
}

/// The `--settings` a Work run starts with. The shell's sandbox is the
/// boundary: a sandboxed command runs without asking, and one that asks
/// to leave it, or a connection to a host the grant does not list, comes
/// to Perch as a request rather than being refused. No sandbox means no
/// run. Writes go to the worktree, the temporary directory and the
/// grant, never to the protected paths in the worktree or a `.git` below
/// its top nor, unless the reader lets Work commit, to the git
/// directories it uses; with that, the git directories are writable but
/// their config and hooks are not (see [`commit_paths`]).
pub fn claude_work_settings(cwd: &std::path::Path, grant: &WorkGrant) -> String {
    let real = real_path(cwd);
    let mut allow_write = vec![real.clone(), real_path(&std::env::temp_dir())];
    allow_write.extend(grant.write.iter().map(|p| real_path(p)));
    // A `.git` directory is where a commit writes, so the commit grant
    // opens it; a linked worktree's `.git` file only points elsewhere,
    // and stays read-only whatever is granted.
    let git_is_dir = real.join(".git").is_dir();
    let mut deny_write: Vec<PathBuf> = WORK_PROTECTED
        .iter()
        .filter(|name| !(grant.commit && git_is_dir && **name == ".git"))
        .map(|name| real.join(name))
        .collect();
    deny_write.push(nested_git(&real));
    if grant.commit {
        let (dirs, guarded) = commit_paths(&real);
        allow_write.extend(dirs);
        deny_write.extend(guarded);
    } else {
        deny_write.extend(linked_git_dirs(&real));
    }
    serde_json::json!({
        "sandbox": {
            "enabled": true,
            "autoAllowBashIfSandboxed": true,
            "allowUnsandboxedCommands": true,
            "failIfUnavailable": true,
            "filesystem": {
                "allowWrite": allow_write,
                "denyWrite": deny_write,
            },
            "network": {
                "allowedDomains": grant.network,
            },
        },
    })
    .to_string()
}

/// The permission profile a Codex Work run runs under, defined in the
/// config its thread starts with.
pub const CODEX_WORK_PROFILE: &str = "perch_work";

/// Codex's Work profile, path by path: everything readable, the
/// workspace, the temporary directory and the grant writable, and the
/// protected paths in the worktree and the git directories it uses read
/// only. Codex's `workspace-write` is not used, because for a linked
/// worktree it makes the repository's git directory writable; a path
/// named here outranks the workspace it is in. The profile names no
/// network, and a profile without one has none: a command that needs it
/// asks.
///
/// A `.git` below the worktree's top is denied (see [`nested_git`]).
/// Codex takes a glob only as `deny`, which stops reads as well as
/// writes; its `**/` matches no directory too, so without the `*/` the
/// glob would hide the top-level `.git` from git itself. The deny holds
/// against a bare `git init` or `git clone`, which Codex 0.156 let past
/// the profile the way it did `git commit`.
///
/// Codex 0.159 holds a bare `git commit` to the profile, and runs a
/// command it was let run outside the sandbox inside it all the same,
/// so the commit grant is the only way a Codex run commits: it opens the
/// git directories, as [`commit_paths`] does for Claude Code. Here the
/// more specific path wins, so `worktrees/` is read-only and the run's
/// own directory in it is not, which also keeps a worktree added later
/// read-only without a glob.
pub fn codex_work_entries(cwd: &std::path::Path, grant: &WorkGrant) -> Vec<(String, &'static str)> {
    let real = real_path(cwd);
    let mut entries = vec![
        (":root".to_string(), "read"),
        (":workspace_roots".to_string(), "write"),
        (":tmpdir".to_string(), "write"),
    ];
    for path in &grant.write {
        entries.push((real_path(path).to_string_lossy().into_owned(), "write"));
    }
    let git_is_dir = real.join(".git").is_dir();
    for name in WORK_PROTECTED
        .iter()
        .filter(|name| !(grant.commit && git_is_dir && **name == ".git"))
    {
        entries.push((real.join(name).to_string_lossy().into_owned(), "read"));
    }
    if grant.commit {
        let dirs = git_dirs(&real);
        let common = dirs.last().expect("one git directory at least");
        let guarded = dirs
            .iter()
            .flat_map(|dir| GIT_CONTROL.map(|name| dir.join(name)))
            .chain([common.join("worktrees"), common.join("modules")]);
        entries.extend(
            dirs.iter()
                .map(|dir| (dir.to_string_lossy().into_owned(), "write")),
        );
        entries.extend(guarded.map(|path| (path.to_string_lossy().into_owned(), "read")));
    } else {
        for dir in linked_git_dirs(&real) {
            entries.push((dir.to_string_lossy().into_owned(), "read"));
        }
    }
    entries.push((nested_git(&real).to_string_lossy().into_owned(), "deny"));
    entries
}

/// The MCP servers the user's own Codex config declares, which a Work
/// run turns off: `codex app-server` has no `--ignore-user-config`, and
/// those servers run outside any sandbox.
pub fn codex_user_mcp_servers() -> Vec<String> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")));
    let Some(text) = home.and_then(|h| std::fs::read_to_string(h.join("config.toml")).ok()) else {
        return Vec::new();
    };
    let Ok(value) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    value
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .map(|servers| servers.keys().cloned().collect())
        .unwrap_or_default()
}

/// The arguments that keep a Claude Code run to the user's own settings:
/// only `~/.claude/settings.json` is read, never the repository's
/// `.claude/settings*.json` or `.mcp.json`, and no MCP server starts.
pub fn claude_isolation_args(cwd: &std::path::Path) -> Vec<String> {
    isolation_args_with(claude_settings(cwd))
}

/// For a repository the reader trusts: its own settings and local
/// settings are read beside the user's, and its `.mcp.json`, when it has
/// one, is the MCP config.
fn trusted_args_with(cwd: &std::path::Path, settings: String) -> Vec<String> {
    let mcp = cwd.join(".mcp.json");
    let mcp = if mcp.is_file() {
        mcp.to_string_lossy().into_owned()
    } else {
        r#"{"mcpServers":{}}"#.to_string()
    };
    vec![
        "--setting-sources".into(),
        "user,project,local".into(),
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        mcp,
        "--settings".into(),
        settings,
    ]
}

/// [`claude_isolation_args`] with the sandbox a mode needs.
fn isolation_args_with(settings: String) -> Vec<String> {
    vec![
        "--setting-sources".into(),
        "user".into(),
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        r#"{"mcpServers":{}}"#.into(),
        "--settings".into(),
        settings,
    ]
}

/// The path with symlinks resolved, as the sandbox and Codex see it, or
/// the path as given when it cannot be resolved.
fn real_path(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether an environment variable is kept from the agent. GitHub's
/// tokens, the one Perch's own git commands carry, and anything named
/// like a secret: the process runs a model on text nobody vetted, and
/// what it cannot see it cannot be talked into repeating. The credentials
/// the CLIs themselves sign in with are the exception, since without them
/// there is no run; other providers' credentials are not, so a CLI
/// configured that way has to be signed in instead.
pub fn is_scrubbed_env(name: &str) -> bool {
    const KEPT: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
    ];
    const ALWAYS: &[&str] = &["GITHUB_TOKEN", "GH_TOKEN", "PERCH_GIT_TOKEN"];
    const SUFFIXES: &[&str] = &[
        "_TOKEN",
        "_SECRET",
        "_KEY",
        "_APIKEY",
        "_PASSWORD",
        "_PASSWD",
        "_PASS",
        "_PAT",
        "_CREDENTIAL",
        "_CREDENTIALS",
    ];
    let name = name.to_ascii_uppercase();
    if KEPT.contains(&name.as_str()) {
        return false;
    }
    ALWAYS.contains(&name.as_str()) || SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// Drop every scrubbed variable from a command's inherited environment.
fn scrub_env(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(is_scrubbed_env) {
            command.env_remove(name);
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunSpec {
    pub backend: Backend,
    /// The executable: `claude` or `codex`, or a path from config.
    pub binary: PathBuf,
    /// `sonnet`, `opus`, or a full model id.
    pub model: String,
    pub effort: Effort,
    pub cwd: PathBuf,
    pub prompt: String,
    /// Appended to the agent's system prompt. First run of a thread only.
    pub system: Option<String>,
    /// A session id from an earlier run, to continue it.
    pub resume: Option<String>,
    /// Ask or Work. For Claude Code it picks the tool lists and the
    /// sandbox settings; Codex has no tool list, and it picks the sandbox.
    pub mode: Mode,
}

impl RunSpec {
    /// The command line, for the log and for tests.
    pub fn args(&self) -> Vec<String> {
        match self.backend {
            Backend::ClaudeCode => self.claude_args(),
            Backend::Codex => self.codex_args(),
        }
    }

    /// The prompt goes last, after `--`, so a question that begins with a
    /// dash is not read as an option. Both CLIs reject one otherwise.
    fn claude_args(&self) -> Vec<String> {
        let work = matches!(self.mode, Mode::Work(_));
        let mut args = vec![
            "-p".to_string(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
        ];
        // The prompt and anything the reader says while the run goes go
        // on stdin, and each message comes back as the agent reads it. A
        // Work run can also ask: anything its permissions would prompt
        // for comes to Perch.
        args.extend([
            "--input-format".into(),
            "stream-json".into(),
            "--replay-user-messages".into(),
        ]);
        if work {
            args.extend(["--permission-prompt-tool".into(), "stdio".into()]);
        }
        args.extend([
            "--model".into(),
            self.model.clone(),
            "--effort".into(),
            self.effort.claude_name().into(),
            "--permission-mode".into(),
            match &self.mode {
                Mode::Work(grant) if grant.auto_review => "auto".into(),
                _ => "default".into(),
            },
        ]);
        let (allowed, denied): (Vec<String>, Vec<String>) = match &self.mode {
            Mode::Ask => {
                args.extend(claude_isolation_args(&self.cwd));
                (
                    READ_ONLY_TOOLS.iter().map(|t| t.to_string()).collect(),
                    denied_tools().collect(),
                )
            }
            Mode::Work(grant) => {
                let settings = claude_work_settings(&self.cwd, grant);
                if grant.trust {
                    args.extend(trusted_args_with(&self.cwd, settings));
                } else {
                    args.extend(isolation_args_with(settings));
                }
                let (edits, protected) = work_edit_rules(&self.cwd);
                (
                    WORK_TOOLS
                        .iter()
                        .map(|t| t.to_string())
                        .chain(edits)
                        .collect(),
                    NETWORK_TOOLS
                        .iter()
                        .map(|t| t.to_string())
                        .chain(protected)
                        .collect(),
                )
            }
        };
        if let Some(system) = &self.system {
            args.push("--append-system-prompt".into());
            args.push(system.clone());
        }
        if let Some(session) = &self.resume {
            args.push("--resume".into());
            args.push(session.clone());
        }
        args.push("--allowedTools".into());
        args.extend(allowed);
        if self.effort == Effort::Ultra {
            args.extend(ORCHESTRATION_TOOLS.iter().map(|t| t.to_string()));
        }
        args.push("--disallowedTools".into());
        args.extend(denied);
        args
    }

    /// The conversation the run holds with its CLI over stdin.
    pub(crate) fn conversation(&self) -> Conversation {
        match self.backend {
            Backend::ClaudeCode => Conversation::Claude(ClaudeHost::new(self.prompt.clone())),
            Backend::Codex => Conversation::Codex(Box::new(CodexServer::new(self.codex_thread()))),
        }
    }

    /// What a Codex thread starts with: the directory, the model and
    /// effort, the worktree's trust, and the user's MCP servers turned
    /// off. A Work thread has the Work profile and asks; a question's is
    /// read-only and never asks.
    fn codex_thread(&self) -> CodexThread {
        let grant = match &self.mode {
            Mode::Work(grant) => Some(grant),
            Mode::Ask => None,
        };
        let trust = if grant.is_some_and(|g| g.trust) {
            "trusted"
        } else {
            "untrusted"
        };
        let mut projects = serde_json::Map::new();
        projects.insert(
            self.cwd.to_string_lossy().into_owned(),
            serde_json::json!({"trust_level": trust}),
        );
        projects.insert(
            real_path(&self.cwd).to_string_lossy().into_owned(),
            serde_json::json!({"trust_level": trust}),
        );
        let mcp: serde_json::Map<String, serde_json::Value> = codex_user_mcp_servers()
            .into_iter()
            .map(|name| (name, serde_json::json!({"enabled": false})))
            .collect();
        let mut config = serde_json::json!({
            "projects": projects,
            "model_reasoning_effort": self.effort.name(),
            "mcp_servers": mcp,
        });
        if let Some(grant) = grant {
            let filesystem: serde_json::Map<String, serde_json::Value> =
                codex_work_entries(&self.cwd, grant)
                    .into_iter()
                    .map(|(path, access)| (path, serde_json::json!(access)))
                    .collect();
            config["default_permissions"] = serde_json::json!(CODEX_WORK_PROFILE);
            config["permissions"] =
                serde_json::json!({CODEX_WORK_PROFILE: {"filesystem": filesystem}});
        }
        CodexThread {
            cwd: self.cwd.to_string_lossy().into_owned(),
            model: self.model.clone(),
            effort: self.effort.name().to_string(),
            prompt: self.prompt.clone(),
            developer_instructions: self.system.clone(),
            resume: self.resume.clone(),
            approval_policy: if grant.is_some() {
                "on-request"
            } else {
                "never"
            }
            .to_string(),
            reviewer: grant.map(|g| if g.auto_review { "auto_review" } else { "user" }.to_string()),
            sandbox: grant.is_none().then(|| "read-only".to_string()),
            config,
        }
    }

    /// `codex app-server`: no plugins, hooks or apps, and each of the
    /// user's MCP servers off, by name, where the name can be a `-c` key;
    /// the thread's config turns them all off as well. The MCP servers
    /// would run outside the sandbox, and a prompt could call them.
    fn codex_args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "app-server",
            "--disable",
            "plugins",
            "--disable",
            "hooks",
            "--disable",
            "apps",
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();
        for name in codex_user_mcp_servers() {
            if name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                args.extend(["-c".into(), format!("mcp_servers.{name}.enabled=false")]);
            }
        }
        args
    }
}

/// `s` as a TOML basic string, quotes included, for `codex -c key=value`,
/// which parses the value as TOML.
pub fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `path` as a prompt names it: `~/…` when it is under the home directory.
/// An agent retypes a path it is given, and retyping the user's name in
/// full got it wrong (`kumarujjawwal` for `kumarujjawal`, so `cat` found
/// nothing). Both agents' shells and Claude's file tools expand the `~`.
pub fn prompt_path(path: &std::path::Path) -> String {
    prompt_path_in(path, std::env::var_os("HOME").map(PathBuf::from).as_deref())
}

/// `prompt_path` with the home passed in, so it can be tested.
pub fn prompt_path_in(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    match home
        .filter(|h| h.is_absolute())
        .and_then(|h| path.strip_prefix(h).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// One parser per backend. Claude associates an explicit permission denial
/// with the terminal result it caused; Codex remembers the thread and which
/// commands have begun.
enum ParserKind {
    Claude {
        pending_permission_denial: bool,
        terminal_permission_denial: Option<bool>,
    },
    Codex(CodexParser),
}

/// Where a run's raw protocol output is kept, line for line, beside the
/// events made from it. Diagnostic only: nothing reads it back, and a
/// file that cannot be opened costs a warning, never the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    /// Every stdout line, the CLI's own JSON, as it arrived.
    pub stdout: PathBuf,
    /// Every stderr line.
    pub stderr: PathBuf,
}

/// One transcript file, flushed after every line so a crash keeps what
/// came before it. The writes are small and local; they run inline.
struct LineLog(std::io::BufWriter<std::fs::File>);

impl LineLog {
    fn create(path: &std::path::Path) -> Option<Self> {
        match std::fs::File::create(path) {
            Ok(file) => Some(Self(std::io::BufWriter::new(file))),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not open a run transcript");
                None
            }
        }
    }

    fn line(&mut self, line: &str) {
        use std::io::Write;
        let written = self
            .0
            .write_all(line.as_bytes())
            .and_then(|()| self.0.write_all(b"\n"))
            .and_then(|()| self.0.flush());
        if let Err(error) = written {
            tracing::warn!(%error, "could not write a run transcript line");
        }
    }
}

struct Parser {
    kind: ParserKind,
    /// Every stdout line, copied before it is parsed, when asked for.
    transcript: Option<LineLog>,
    /// For a run on stdin: what to write back, and when to stop.
    conversation: Option<Conversation>,
    /// Lines the conversation wants written, drained by the pump.
    outbox: Vec<String>,
    /// The conversation is over: close stdin.
    close: bool,
    /// A `Started` has gone out. Claude Code starts each turn afresh, and
    /// a steer can make more than one, but the run starts once.
    started: bool,
}

impl Parser {
    fn for_backend(backend: Backend) -> Self {
        let kind = match backend {
            Backend::ClaudeCode => ParserKind::Claude {
                pending_permission_denial: false,
                terminal_permission_denial: None,
            },
            Backend::Codex => ParserKind::Codex(CodexParser::new()),
        };
        Self {
            kind,
            transcript: None,
            conversation: None,
            outbox: Vec::new(),
            close: false,
            started: false,
        }
    }

    fn parse_line(&mut self, line: &str) -> Result<Vec<RunEvent>, String> {
        let mut events = self.parse_turn_line(line)?;
        let started = self.started;
        events.retain(|event| !(started && matches!(event, RunEvent::Started { .. })));
        self.started |= events.iter().any(|e| matches!(e, RunEvent::Started { .. }));
        Ok(events)
    }

    fn parse_turn_line(&mut self, line: &str) -> Result<Vec<RunEvent>, String> {
        if let Some(transcript) = &mut self.transcript {
            transcript.line(line);
        }
        if line.trim().is_empty() {
            return Ok(Vec::new());
        }
        let value = serde_json::from_str::<serde_json::Value>(line)
            .map_err(|error| format!("The agent emitted malformed JSON: {error}"))?;
        usage::observe(self.backend(), &value);
        if let Some(conversation) = self.conversation.as_mut() {
            let reply = conversation.after_line(&value);
            self.outbox.extend(reply.writes);
            self.close |= reply.close;
            if conversation.parses_everything() {
                return Ok(reply.events);
            }
            let mut events = self.parse_print_line(line, &value, reply.more);
            events.extend(reply.events);
            return Ok(events);
        }
        Ok(self.parse_print_line(line, &value, false))
    }

    /// Queue what gives the agent the reader's message, for a run on
    /// stdin.
    fn steer(&mut self, text: &str) {
        if let Some(conversation) = self.conversation.as_mut() {
            self.outbox.extend(conversation.steer(text));
        }
    }

    /// `more`: the line ends a turn and another follows, so a result in
    /// it is not the run's.
    fn parse_print_line(
        &mut self,
        line: &str,
        value: &serde_json::Value,
        more: bool,
    ) -> Vec<RunEvent> {
        match &mut self.kind {
            ParserKind::Claude {
                pending_permission_denial,
                terminal_permission_denial,
            } => {
                let mut events = parse_line(line);
                if more {
                    events.retain(|event| !matches!(event, RunEvent::Finished { .. }));
                }
                if terminal_permission_denial.is_none() {
                    if parse::is_explicit_permission_denial(line) {
                        *pending_permission_denial = true;
                    } else if value.get("type").and_then(serde_json::Value::as_str)
                        == Some("assistant")
                        && value
                            .get("parent_tool_use_id")
                            .and_then(serde_json::Value::as_str)
                            .is_none()
                    {
                        // A new top-level assistant turn means Claude recovered
                        // from the denied tool. A later failure belongs to that
                        // work, not to the earlier denial.
                        *pending_permission_denial = false;
                    }
                    if events
                        .iter()
                        .any(|event| matches!(event, RunEvent::Failed { .. }))
                    {
                        *terminal_permission_denial = Some(*pending_permission_denial);
                    } else if events.iter().any(|event| {
                        matches!(event, RunEvent::Finished { .. } | RunEvent::Stopped { .. })
                    }) {
                        *terminal_permission_denial = Some(false);
                    }
                }
                events
            }
            ParserKind::Codex(p) => p.parse_line(line),
        }
    }

    fn backend(&self) -> Backend {
        match self.kind {
            ParserKind::Claude { .. } => Backend::ClaudeCode,
            ParserKind::Codex(_) => Backend::Codex,
        }
    }

    fn permission_denied(&self) -> bool {
        matches!(
            self.kind,
            ParserKind::Claude {
                terminal_permission_denial: Some(true),
                ..
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    Started {
        session_id: String,
        model: String,
    },
    /// Something the agent said. One per message, not per token.
    Text(String),
    /// The agent read a message the reader sent while it worked, by
    /// `Run::steer`, as it was sent. It comes where the message landed
    /// among the steps.
    Steered(String),
    ToolCall {
        id: String,
        name: String,
        summary: String,
        /// The delegation tool call that owns this activity, when the CLI
        /// identifies it. `None` is a top-level step.
        parent_id: Option<String>,
    },
    ToolResult {
        id: String,
        ok: bool,
        summary: String,
    },
    /// The agent asks to do something its sandbox would not let it. The
    /// run waits until `Run::answer` is called with `id`.
    Approval {
        id: String,
        request: ApprovalRequest,
    },
    /// A structured denial reported by the CLI. It is activity rather than
    /// necessarily terminal: an agent may recover and complete after it.
    PermissionDenied {
        tool: Option<String>,
        message: String,
    },
    /// A report the agent filed through a tool instead of saying it. Claude
    /// Code's review skill hands its findings to `ReportFindings` and keeps
    /// them out of the prose, so the prose alone says "(none)". The value
    /// is the call's input, verbatim; the matching `ToolCall` still arrives
    /// as activity. `parent_id` is set when a subagent filed it.
    Report {
        tool: String,
        input: serde_json::Value,
        parent_id: Option<String>,
    },
    /// Token counts reported by the CLI for this turn. Missing counters stay
    /// `None` rather than being mistaken for zero.
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
    },
    Finished {
        session_id: String,
        cost_usd: Option<f64>,
        duration_ms: u64,
    },
    Failed {
        message: String,
    },
    /// A requested stop, published only after the process tree has been
    /// reaped. `forced` means the graceful interval ended in SIGKILL.
    Stopped {
        forced: bool,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid review invocation: {0}")]
    InvalidReview(String),
    #[error("could not start {binary}: {source}")]
    Spawn {
        binary: PathBuf,
        source: std::io::Error,
    },
    #[error("could not attach the parent-lifetime watchdog to {binary}: {source}")]
    Watchdog {
        binary: PathBuf,
        source: std::io::Error,
    },
}

/// Why a process-backed run failed.
///
/// Kept separate from the human-readable message so callers never need to
/// infer lifecycle state from wording printed by either CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFailureKind {
    /// The agent emitted an explicit failed result.
    Reported,
    /// The CLI explicitly reported a structured permission denial and then
    /// failed the run.
    PermissionDenied,
    /// Its stdout could not be read.
    Output,
    /// The process exited without a valid successful result, or exited
    /// unsuccessfully after reporting one.
    UnexpectedExit,
    /// Waiting for the process itself failed.
    Wait,
    /// The event consumer disappeared while the process was active.
    ConsumerGone,
}

/// The terminal result of a run, available only after its child has been
/// reaped and its process group has been cleaned up.
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    Finished {
        session_id: String,
        cost_usd: Option<f64>,
        duration_ms: u64,
    },
    Failed {
        kind: RunFailureKind,
        message: String,
    },
    /// `forced` says the process group did not leave during the graceful
    /// stop interval and received SIGKILL.
    Stopped { forced: bool },
}

#[derive(Debug)]
struct Cancellation {
    requested: AtomicBool,
    wake: Notify,
}

impl Cancellation {
    fn request(&self) {
        if !self.requested.swap(true, Ordering::AcqRel) {
            self.wake.notify_one();
        }
    }
}

/// A detached Unix watchdog owns the read end of this pipe. The normal run
/// writes one byte before closing; abrupt process death closes the descriptor
/// without that byte, and the watchdog terminates the agent process group.
#[derive(Debug)]
struct ParentWatchdog {
    #[cfg(unix)]
    lifetime: OwnedFd,
}

impl ParentWatchdog {
    #[cfg(unix)]
    fn arm(process_group: Option<u32>) -> std::io::Result<Self> {
        let process_group =
            process_group.ok_or_else(|| std::io::Error::other("the child has no process id"))?;
        let mut pipe = [-1; 2];
        // SAFETY: `pipe` points to two valid integers. Both descriptors are
        // closed on every error path below.
        if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if let Err(error) = set_close_on_exec(pipe[0]).and_then(|()| set_close_on_exec(pipe[1])) {
            close_fd(pipe[0]);
            close_fd(pipe[1]);
            return Err(error);
        }

        // Find a tight upper bound before forking. The child only invokes
        // async-signal-safe libc calls, avoiding allocator/runtime locks that
        // may be held by another thread at `fork` time.
        let close_through = highest_open_fd().saturating_add(256);
        // SAFETY: after `fork`, the child calls only async-signal-safe libc
        // functions and ends with `_exit`; Rust destructors never run there.
        let first = unsafe { libc::fork() };
        if first < 0 {
            let error = std::io::Error::last_os_error();
            close_fd(pipe[0]);
            close_fd(pipe[1]);
            return Err(error);
        }
        if first == 0 {
            // SAFETY: this branch is the isolated post-fork child described
            // above and never returns to Rust or the Tokio runtime.
            unsafe {
                watchdog_fork_child(
                    pipe[0],
                    pipe[1],
                    process_group as libc::pid_t,
                    close_through,
                )
            }
        }

        close_fd(pipe[0]);
        let mut status = 0;
        let waited = loop {
            // SAFETY: `first` is the direct child just created and `status`
            // is valid writable storage.
            let result = unsafe { libc::waitpid(first, &mut status, 0) };
            if result >= 0 {
                break result;
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                break result;
            }
        };
        if waited < 0 || !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 {
            let error = if waited < 0 {
                std::io::Error::last_os_error()
            } else {
                std::io::Error::other("the parent-lifetime watchdog could not detach")
            };
            close_fd(pipe[1]);
            return Err(error);
        }

        // SAFETY: the parent exclusively owns the still-open write end now.
        let lifetime = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
        Ok(Self { lifetime })
    }

    #[cfg(not(unix))]
    fn arm(_process_group: Option<u32>) -> std::io::Result<Self> {
        Ok(Self {})
    }

    /// Tell the watchdog that the process group was intentionally reaped.
    /// Dropping without calling this method is interpreted as a crash.
    #[cfg(unix)]
    fn disarm(self) {
        use std::os::fd::AsRawFd;

        let byte = [1_u8];
        loop {
            // SAFETY: `lifetime` remains open for this call and the pointer
            // addresses the one-byte buffer.
            let written =
                unsafe { libc::write(self.lifetime.as_raw_fd(), byte.as_ptr().cast(), byte.len()) };
            if written >= 0
                || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                break;
            }
        }
        // `self` closes the descriptor here. The detached watchdog consumes
        // the byte and exits without signalling the already-clean group.
    }

    #[cfg(not(unix))]
    fn disarm(self) {}
}

#[cfg(unix)]
fn set_close_on_exec(fd: libc::c_int) -> std::io::Result<()> {
    // SAFETY: `fd` is an open pipe descriptor owned by this function's caller.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn close_fd(fd: libc::c_int) {
    // SAFETY: closing an owned raw descriptor is safe; errors need no action
    // during cleanup.
    unsafe {
        libc::close(fd);
    }
}

#[cfg(unix)]
fn highest_open_fd() -> libc::c_int {
    // `/dev/fd` includes the directory handle used for this scan, so the
    // result covers every descriptor open before `fork`. A cushion covers a
    // short concurrent-open race without walking the million-entry fd limit.
    std::fs::read_dir("/dev/fd")
        .or_else(|_| std::fs::read_dir("/proc/self/fd"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .parse::<libc::c_int>()
                .ok()
        })
        .max()
        .unwrap_or(1024)
}

/// Detach a grandchild which can outlive the Perch process. This function is
/// entered after `fork` in a multi-threaded program, so it must never allocate,
/// unwind, or call back into Rust/Tokio.
#[cfg(unix)]
unsafe fn watchdog_fork_child(
    read_fd: libc::c_int,
    write_fd: libc::c_int,
    process_group: libc::pid_t,
    close_through: libc::c_int,
) -> ! {
    // SAFETY: all operations in this function are async-signal-safe libc
    // primitives, and descriptor/pid values came from successful syscalls.
    unsafe {
        libc::close(write_fd);
        if libc::setsid() < 0 {
            libc::_exit(120);
        }
        let second = libc::fork();
        if second < 0 {
            libc::_exit(121);
        }
        if second > 0 {
            libc::_exit(0);
        }

        let mut fd = 0;
        while fd <= close_through {
            if fd != read_fd {
                libc::close(fd);
            }
            fd += 1;
        }

        let mut byte = 0_u8;
        let read = libc::read(read_fd, (&mut byte as *mut u8).cast(), 1);
        if read == 1 {
            // Normal completion sent the disarm byte.
            libc::_exit(0);
        }

        // EOF (or an unusable marker pipe) means the owning Perch process
        // vanished. Give cooperative descendants the same grace interval as
        // explicit cancellation, then kill whatever remains in the group.
        libc::kill(-process_group, libc::SIGTERM);
        let pause = libc::timespec {
            tv_sec: 0,
            tv_nsec: 10_000_000,
        };
        let mut checks = 0;
        while checks < 75 {
            if libc::kill(-process_group, 0) != 0 {
                libc::_exit(0);
            }
            libc::nanosleep(&pause, std::ptr::null_mut());
            checks += 1;
        }
        libc::kill(-process_group, libc::SIGKILL);
        libc::_exit(0);
    }
}

/// A running agent. Dropping it does not stop the process; `cancel` does.
/// `wait` and `cancel_and_wait` may be called from any clone and always
/// return the same terminal result.
#[derive(Debug, Clone)]
pub struct Run {
    cancel: Arc<Cancellation>,
    outcome: watch::Receiver<Option<RunOutcome>>,
    /// Where answers to approval requests and the reader's steers go,
    /// for a run on stdin.
    inputs: Option<UnboundedSender<Input>>,
}

/// What the reader gives a run on stdin while it goes.
#[derive(Debug)]
enum Input {
    Answer(String, Decision),
    Steer(String),
}

impl Run {
    /// Start the process and read its events onto `events` until it ends.
    ///
    /// Must be called on a tokio runtime. Exactly one of `Finished`,
    /// `Failed`, and `Stopped` is the last event sent.
    pub fn spawn(spec: RunSpec, events: UnboundedSender<RunEvent>) -> Result<Run, Error> {
        let args = spec.args();
        let conversation = Some(spec.conversation());
        Self::spawn_process(
            spec.backend,
            spec.binary,
            spec.cwd,
            args,
            None,
            conversation,
            events,
        )
    }

    /// Answer approval request `id`. False when the run cannot ask, or
    /// has already ended.
    pub fn answer(&self, id: &str, decision: Decision) -> bool {
        self.send(Input::Answer(id.to_string(), decision))
    }

    /// Tell the agent something more while it works. It reads it once its
    /// current step is done, and a `RunEvent::Steered` says where. False
    /// when the run has stopped taking input: it is a review, or its
    /// last turn has ended. A steer that goes in just as the run ends
    /// may still never be read; no `Steered` comes for it.
    pub fn steer(&self, text: &str) -> bool {
        self.send(Input::Steer(text.to_string()))
    }

    fn send(&self, input: Input) -> bool {
        match &self.inputs {
            Some(inputs) => inputs.send(input).is_ok(),
            None => false,
        }
    }

    /// Start an installed CLI's native local code-review operation using
    /// the same process-tree lifetime, cancellation, event parsing, and
    /// watchdog guarantees as an ordinary question.
    pub fn spawn_review(spec: ReviewSpec, events: UnboundedSender<RunEvent>) -> Result<Run, Error> {
        spec.validate().map_err(Error::InvalidReview)?;
        let args = spec.args();
        let transcript = spec.transcript.clone();
        Self::spawn_process(
            spec.backend,
            spec.binary,
            spec.cwd,
            args,
            transcript,
            None,
            events,
        )
    }

    /// Start a structured prior-finding verification pass without inheriting
    /// repository instructions, hooks, plugins, MCP servers, or approvals.
    pub fn spawn_review_verification(
        spec: ReviewVerificationSpec,
        events: UnboundedSender<RunEvent>,
    ) -> Result<Run, Error> {
        spec.validate().map_err(Error::InvalidReview)?;
        let args = spec.args();
        let transcript = spec.transcript.clone();
        Self::spawn_process(
            spec.backend,
            spec.binary,
            spec.cwd,
            args,
            transcript,
            None,
            events,
        )
    }

    fn spawn_process(
        backend: Backend,
        binary: PathBuf,
        cwd: PathBuf,
        args: Vec<String>,
        transcript: Option<Transcript>,
        mut conversation: Option<Conversation>,
        events: UnboundedSender<RunEvent>,
    ) -> Result<Run, Error> {
        let started_at = Instant::now();
        let mut command = Command::new(&binary);
        command
            .args(args)
            .current_dir(&cwd)
            // A run with a conversation holds it on stdin; a review gets
            // nothing there.
            .stdin(if conversation.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        scrub_env(&mut command);
        // Put every CLI invocation in its own process group. Claude and
        // Codex may launch commands and subagents of their own; signalling
        // the group keeps those descendants under the same lifetime.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|source| Error::Spawn {
            binary: binary.clone(),
            source,
        })?;

        let process_group = child.id();
        let watchdog = match ParentWatchdog::arm(process_group) {
            Ok(watchdog) => watchdog,
            Err(source) => {
                // Never return a live, unowned process when crash protection
                // could not be installed. Reaping continues on the runtime.
                kill_group(process_group);
                let _ = child.start_kill();
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
                return Err(Error::Watchdog { binary, source });
            }
        };
        let cancel = Arc::new(Cancellation {
            requested: AtomicBool::new(false),
            wake: Notify::new(),
        });
        let (outcome_tx, outcome) = watch::channel(None);
        let (inputs_tx, inputs_rx) = tokio::sync::mpsc::unbounded_channel();
        let opening = conversation
            .as_mut()
            .map(Conversation::opening)
            .unwrap_or_default();
        let mut io = Io {
            stdin: child.stdin.take(),
            inputs: conversation.is_some().then_some(inputs_rx),
        };
        let run = Run {
            cancel: cancel.clone(),
            outcome,
            inputs: conversation.is_some().then_some(inputs_tx),
        };

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let (stdout_log, stderr_log) = match &transcript {
                Some(transcript) => (
                    LineLog::create(&transcript.stdout),
                    LineLog::create(&transcript.stderr),
                ),
                None => (None, None),
            };
            let mut stderr_task = tokio::spawn(read_stderr_tail(stderr, stderr_log));
            let mut parser = Parser::for_backend(backend);
            parser.transcript = stdout_log;
            parser.conversation = conversation;
            parser.outbox = opening;
            io.flush(&mut parser).await;
            let exit = drive_until_exit(
                &mut child,
                process_group,
                &cancel,
                &mut lines,
                &mut parser,
                &events,
                started_at,
                &mut io,
            )
            .await;

            // `drive_until_exit` has reaped the direct child and cleaned its
            // process group. A byte distinguishes this normal disarm from
            // EOF caused by the Perch process being killed.
            watchdog.disarm();

            // The child and its group are gone now, so inherited stderr
            // descriptors should close. An escaped descendant must not hold
            // completion hostage; its output is diagnostic only.
            let stderr = match tokio::time::timeout(STDERR_DRAIN, &mut stderr_task).await {
                Ok(Ok(text)) => text,
                _ => {
                    stderr_task.abort();
                    String::new()
                }
            };
            let outcome = outcome_from(exit, &stderr);
            let terminal = terminal_event(&outcome);
            let _ = events.send(terminal);
            outcome_tx.send_replace(Some(outcome));
        });

        Ok(run)
    }

    /// Ask the process tree to stop. A typed `RunEvent::Stopped` is the last
    /// stream event; `wait` returns the same state after reaping.
    pub fn cancel(&self) {
        self.cancel.request();
    }

    /// Wait until the process has been reaped and its process group cleaned
    /// up. Safe to call more than once and from cloned handles.
    pub async fn wait(&self) -> RunOutcome {
        let mut outcome = self.outcome.clone();
        loop {
            if let Some(done) = outcome.borrow().clone() {
                return done;
            }
            if outcome.changed().await.is_err() {
                return RunOutcome::Failed {
                    kind: RunFailureKind::Wait,
                    message: "The agent runner ended without a result".into(),
                };
            }
        }
    }

    /// Request a stop and wait for the entire process tree to be gone.
    pub async fn cancel_and_wait(&self) -> RunOutcome {
        self.cancel();
        self.wait().await
    }
}

const STOP_GRACE: std::time::Duration = std::time::Duration::from_millis(750);
const EXIT_DRAIN: std::time::Duration = std::time::Duration::from_secs(2);
const STDERR_DRAIN: std::time::Duration = std::time::Duration::from_millis(250);
const KILL_SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug)]
enum Exit {
    Process {
        status: Result<std::process::ExitStatus, std::io::Error>,
        terminal: Option<RunEvent>,
        permission_denied: bool,
    },
    Stopped {
        forced: bool,
    },
    Output {
        message: String,
    },
    ConsumerGone,
}

enum LineFailure {
    Output(String),
    ConsumerGone,
}

impl From<LineFailure> for Exit {
    fn from(failure: LineFailure) -> Self {
        match failure {
            LineFailure::Output(message) => Exit::Output { message },
            LineFailure::ConsumerGone => Exit::ConsumerGone,
        }
    }
}

/// A run's stdin and the reader's inputs waiting to be written to it.
/// Both are `None` for a run without a conversation.
struct Io {
    stdin: Option<tokio::process::ChildStdin>,
    inputs: Option<UnboundedReceiver<Input>>,
}

impl Io {
    /// The next input, or never for a run with none to wait for.
    async fn next_input(&mut self) -> Option<Input> {
        match self.inputs.as_mut() {
            Some(inputs) => inputs.recv().await,
            None => std::future::pending().await,
        }
    }

    /// Write what the conversation has queued, and close stdin once it is
    /// over. A write that fails means the process is going; its exit says
    /// why.
    async fn flush(&mut self, parser: &mut Parser) {
        use tokio::io::AsyncWriteExt;
        for line in std::mem::take(&mut parser.outbox) {
            if let Some(stdin) = self.stdin.as_mut() {
                let written = async {
                    stdin.write_all(line.as_bytes()).await?;
                    stdin.write_all(b"\n").await?;
                    stdin.flush().await
                }
                .await;
                if let Err(error) = written {
                    tracing::warn!(%error, "could not write to the agent");
                    self.stdin = None;
                }
            }
        }
        if parser.close {
            self.stdin = None;
            self.inputs = None;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive_until_exit(
    child: &mut Child,
    process_group: Option<u32>,
    cancel: &Cancellation,
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    parser: &mut Parser,
    events: &UnboundedSender<RunEvent>,
    started_at: Instant,
    io: &mut Io,
) -> Exit {
    let mut terminal = None;
    loop {
        tokio::select! {
            biased;
            _ = cancel.wake.notified() => {
                let forced = stop_and_reap(child, process_group, lines, parser, events, started_at, &mut terminal).await;
                return Exit::Stopped { forced };
            }
            status = child.wait() => {
                if let Some(failure) = drain_stdout(lines, parser, events, started_at, &mut terminal).await {
                    cleanup_survivors(process_group).await;
                    return failure.into();
                }
                cleanup_survivors(process_group).await;
                return Exit::Process { status, terminal, permission_denied: parser.permission_denied() };
            }
            input = io.next_input() => match input {
                Some(Input::Answer(id, decision)) => {
                    if let Some(line) = parser.conversation.as_mut().and_then(|c| c.answer(&id, &decision)) {
                        parser.outbox.push(line);
                    }
                    io.flush(parser).await;
                }
                Some(Input::Steer(text)) => {
                    parser.steer(&text);
                    io.flush(parser).await;
                }
                None => io.inputs = None,
            },
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    if let Err(failure) = apply_line(parser, &line, events, started_at, &mut terminal) {
                        let exit = failure.into();
                        let _ = stop_and_reap(child, process_group, lines, parser, events, started_at, &mut terminal).await;
                        return exit;
                    }
                    io.flush(parser).await;
                }
                Ok(None) => {
                    // stdout may close before the process does. It still
                    // needs to be reaped, and cancellation remains live.
                    tokio::select! {
                        biased;
                        _ = cancel.wake.notified() => {
                            let forced = stop_and_reap(child, process_group, lines, parser, events, started_at, &mut terminal).await;
                            return Exit::Stopped { forced };
                        }
                        status = child.wait() => {
                            cleanup_survivors(process_group).await;
                            return Exit::Process { status, terminal, permission_denied: parser.permission_denied() };
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "agent stdout unreadable");
                    let _ = stop_and_reap(child, process_group, lines, parser, events, started_at, &mut terminal).await;
                    return Exit::Output { message: format!("Could not read the agent's output: {e}") };
                }
            }
        }
    }
}

/// Parse and publish one line. Terminal protocol events are held until the
/// OS process is reaped, so the UI can trust a terminal row.
fn apply_line(
    parser: &mut Parser,
    line: &str,
    events: &UnboundedSender<RunEvent>,
    started_at: Instant,
    terminal: &mut Option<RunEvent>,
) -> Result<(), LineFailure> {
    let parsed = parser.parse_line(line).map_err(LineFailure::Output)?;
    // Once a protocol terminal has arrived, later protocol output cannot
    // resurrect or amend the run. The bytes are still drained above.
    if terminal.is_some() {
        return Ok(());
    }
    for mut event in parsed {
        if let RunEvent::Finished {
            duration_ms: ms @ 0,
            ..
        } = &mut event
        {
            *ms = started_at.elapsed().as_millis() as u64;
        }
        if matches!(
            event,
            RunEvent::Finished { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. }
        ) {
            *terminal = Some(event);
            continue;
        }
        if events.send(event).is_err() {
            return Err(LineFailure::ConsumerGone);
        }
    }
    Ok(())
}

async fn drain_stdout(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    parser: &mut Parser,
    events: &UnboundedSender<RunEvent>,
    started_at: Instant,
    terminal: &mut Option<RunEvent>,
) -> Option<LineFailure> {
    tokio::time::timeout(EXIT_DRAIN, async {
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if let Err(failure) = apply_line(parser, &line, events, started_at, terminal) {
                        return Some(failure);
                    }
                }
                Ok(None) => return None,
                Err(error) => {
                    return Some(LineFailure::Output(format!(
                        "Could not read the agent's output: {error}"
                    )))
                }
            }
        }
    })
    .await
    .unwrap_or(None)
}

async fn stop_and_reap(
    child: &mut Child,
    process_group: Option<u32>,
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    parser: &mut Parser,
    events: &UnboundedSender<RunEvent>,
    started_at: Instant,
    terminal: &mut Option<RunEvent>,
) -> bool {
    let deadline = tokio::time::Instant::now() + STOP_GRACE;
    terminate_group(process_group);
    let graceful = tokio::time::timeout_at(deadline, async {
        let status = child.wait().await;
        drain_stdout(lines, parser, events, started_at, terminal).await;
        status
    })
    .await;
    if graceful.is_ok() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        wait_for_group(process_group, remaining).await;
        if !group_alive(process_group) {
            return false;
        }
    }

    kill_group(process_group);
    // `start_kill` is also the non-Unix fallback and covers a child that
    // changed its process group after launch. SIGKILL cannot be ignored;
    // waiting here is what prevents zombies and makes `wait` authoritative.
    let _ = child.start_kill();
    let _ = child.wait().await;
    drain_stdout(lines, parser, events, started_at, terminal).await;
    wait_for_group(process_group, KILL_SETTLE).await;
    true
}

/// A normally exited CLI can still have a descendant holding a pipe. Give
/// those descendants a short graceful window and then make the group's
/// lifetime end with the run that created it.
async fn cleanup_survivors(process_group: Option<u32>) {
    if !group_alive(process_group) {
        return;
    }
    terminate_group(process_group);
    let gone = tokio::time::timeout(STOP_GRACE, async {
        while group_alive(process_group) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    if !gone {
        kill_group(process_group);
        wait_for_group(process_group, KILL_SETTLE).await;
    }
}

async fn wait_for_group(process_group: Option<u32>, duration: std::time::Duration) {
    let _ = tokio::time::timeout(duration, async {
        while group_alive(process_group) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
}

fn outcome_from(exit: Exit, stderr: &str) -> RunOutcome {
    match exit {
        Exit::Stopped { forced } => RunOutcome::Stopped { forced },
        Exit::Output { message } => RunOutcome::Failed {
            kind: RunFailureKind::Output,
            message,
        },
        Exit::ConsumerGone => RunOutcome::Failed {
            kind: RunFailureKind::ConsumerGone,
            message: "The agent event consumer went away".into(),
        },
        Exit::Process { status: Err(e), .. } => RunOutcome::Failed {
            kind: RunFailureKind::Wait,
            message: format!("Could not wait for the agent: {e}"),
        },
        Exit::Process {
            status: Ok(status),
            terminal,
            permission_denied,
        } => match terminal {
            Some(RunEvent::Finished {
                session_id,
                cost_usd,
                duration_ms,
            }) if status.success() => RunOutcome::Finished {
                session_id,
                cost_usd,
                duration_ms,
            },
            Some(RunEvent::Failed { message }) => {
                let kind = if permission_denied {
                    RunFailureKind::PermissionDenied
                } else {
                    RunFailureKind::Reported
                };
                RunOutcome::Failed { kind, message }
            }
            Some(RunEvent::Finished { .. }) | None => {
                let tail = stderr_tail_line(stderr);
                let message = if status.success() {
                    "The agent ended without answering".to_string()
                } else {
                    format!(
                        "The agent exited with {status}{}",
                        if tail.is_empty() {
                            String::new()
                        } else {
                            format!(": {tail}")
                        }
                    )
                };
                RunOutcome::Failed {
                    kind: RunFailureKind::UnexpectedExit,
                    message,
                }
            }
            Some(RunEvent::Stopped { forced }) => RunOutcome::Stopped { forced },
            Some(_) => unreachable!("only terminal events are retained"),
        },
    }
}

fn terminal_event(outcome: &RunOutcome) -> RunEvent {
    match outcome {
        RunOutcome::Finished {
            session_id,
            cost_usd,
            duration_ms,
        } => RunEvent::Finished {
            session_id: session_id.clone(),
            cost_usd: *cost_usd,
            duration_ms: *duration_ms,
        },
        RunOutcome::Failed { message, .. } => RunEvent::Failed {
            message: message.clone(),
        },
        RunOutcome::Stopped { forced } => RunEvent::Stopped { forced: *forced },
    }
}

async fn read_stderr_tail(
    stderr: tokio::process::ChildStderr,
    mut transcript: Option<LineLog>,
) -> String {
    use std::collections::VecDeque;
    let mut tail = VecDeque::with_capacity(3);
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(mut line)) = lines.next_line().await {
        if let Some(transcript) = &mut transcript {
            transcript.line(&line);
        }
        if line.chars().count() > 512 {
            line = line.chars().take(511).collect::<String>() + "…";
        }
        if tail.len() == 3 {
            tail.pop_front();
        }
        tail.push_back(line);
    }
    tail.into_iter().collect::<Vec<_>>().join(" ")
}

fn stderr_tail_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(unix)]
fn signal_group(process_group: Option<u32>, signal: i32) {
    let Some(group) = process_group else { return };
    // SAFETY: `kill` is called with a process-group id created for this
    // child. ESRCH only means the group has already left.
    let result = unsafe { libc::kill(-(group as i32), signal) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(group, signal, %error, "could not signal agent process group");
        }
    }
}

#[cfg(unix)]
fn terminate_group(process_group: Option<u32>) {
    signal_group(process_group, libc::SIGTERM);
}

#[cfg(not(unix))]
fn terminate_group(_process_group: Option<u32>) {}

#[cfg(unix)]
fn kill_group(process_group: Option<u32>) {
    signal_group(process_group, libc::SIGKILL);
}

#[cfg(not(unix))]
fn kill_group(_process_group: Option<u32>) {}

#[cfg(unix)]
fn group_alive(process_group: Option<u32>) -> bool {
    let Some(group) = process_group else {
        return false;
    };
    // Signal 0 changes nothing and asks only whether the group exists.
    let result = unsafe { libc::kill(-(group as i32), 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn group_alive(_process_group: Option<u32>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_carries_the_sandbox() {
        let spec = RunSpec {
            backend: Backend::ClaudeCode,
            binary: "claude".into(),
            model: "sonnet".into(),
            effort: Effort::High,
            cwd: "/tmp".into(),
            prompt: "why?".into(),
            system: Some("be brief".into()),
            resume: None,
            mode: Mode::Ask,
        };
        let args = spec.args();
        assert_eq!(args[0], "-p");
        assert!(
            !args.contains(&"why?".to_string()),
            "the prompt goes on stdin"
        );
        assert!(args
            .windows(2)
            .any(|w| w == ["--input-format", "stream-json"]));
        assert!(args.contains(&"--replay-user-messages".to_string()));
        assert!(
            !args.contains(&"--permission-prompt-tool".to_string()),
            "a question never asks"
        );
        assert!(args.windows(2).any(|w| w == ["--effort", "high"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--append-system-prompt", "be brief"]));
        assert!(args.contains(&"Bash(git log:*)".to_string()));
        assert!(
            args.contains(&"Write".to_string()),
            "writing tools are named to be denied"
        );
        assert!(!args
            .iter()
            .any(|a| a.contains("dangerously") || a.contains("bypass")));
        assert!(!args.contains(&"--resume".to_string()));
        assert_isolated_claude(&args, &spec.cwd);
    }

    #[test]
    fn the_network_tools_are_denied_whatever_the_user_allows() {
        let args = claude(Effort::Medium).args();
        let denied = args.iter().position(|a| a == "--disallowedTools").unwrap();
        for tool in ["WebFetch", "WebSearch", "Edit", "Write"] {
            let at = args.iter().position(|a| a == tool).unwrap();
            assert!(denied < at, "{tool} is in the denied list");
        }
    }

    #[test]
    fn a_follow_up_resumes_and_sends_no_system_prompt() {
        let spec = RunSpec {
            backend: Backend::ClaudeCode,
            binary: "claude".into(),
            model: "sonnet".into(),
            effort: Effort::Medium,
            cwd: "/tmp".into(),
            prompt: "and?".into(),
            system: None,
            resume: Some("sess-1".into()),
            mode: Mode::Ask,
        };
        let args = spec.args();
        assert!(args.windows(2).any(|w| w == ["--resume", "sess-1"]));
        assert!(!args.contains(&"--append-system-prompt".to_string()));
        assert_isolated_claude(&args, &spec.cwd);
    }

    /// The repository's settings, MCP servers and shell are kept out of
    /// every Claude Code run, first or resumed.
    fn assert_isolated_claude(args: &[String], cwd: &std::path::Path) {
        assert!(args.windows(2).any(|w| w == ["--setting-sources", "user"]));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(args
            .windows(2)
            .any(|w| w == ["--mcp-config", r#"{"mcpServers":{}}"#]));
        let settings = args
            .windows(2)
            .find(|w| w[0] == "--settings")
            .map(|w| w[1].clone())
            .expect("--settings is passed");
        assert_eq!(settings, claude_settings(cwd));
        let value: serde_json::Value = serde_json::from_str(&settings).unwrap();
        let sandbox = &value["sandbox"];
        assert_eq!(sandbox["enabled"], true);
        assert_eq!(sandbox["autoAllowBashIfSandboxed"], false);
        assert_eq!(sandbox["allowUnsandboxedCommands"], false);
        assert_eq!(sandbox["failIfUnavailable"], true);
        assert_eq!(sandbox["network"]["allowedDomains"], serde_json::json!([]));
        assert_eq!(
            sandbox["network"]["deniedDomains"],
            serde_json::json!(["*"])
        );
        assert_eq!(sandbox["network"]["strictAllowlist"], true);
        let real = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        assert_eq!(
            sandbox["filesystem"]["denyWrite"],
            serde_json::json!([real.to_string_lossy()]),
            "the worktree itself is read-only to the shell"
        );
        assert!(
            !args.iter().any(|a| a == "Bash(rg:*)"),
            "rg --pre runs any program"
        );
        assert_eq!(
            args.iter().filter(|a| *a == "--setting-sources").count(),
            1,
            "one settings source list, so a later one cannot widen it"
        );
        assert!(
            !args
                .windows(2)
                .any(|w| w[0] == "--setting-sources" && w[1].contains("project")),
            "no project settings source"
        );
    }

    fn work(backend: Backend, grant: WorkGrant, resume: Option<&str>) -> RunSpec {
        RunSpec {
            backend,
            binary: "agent".into(),
            model: "m".into(),
            effort: Effort::High,
            cwd: "/tmp".into(),
            prompt: "do it".into(),
            system: None,
            resume: resume.map(str::to_string),
            mode: Mode::Work(grant),
        }
    }

    fn full_grant() -> WorkGrant {
        WorkGrant {
            write: vec!["/opt/cache".into()],
            network: vec!["index.crates.io".into()],
            ..WorkGrant::default()
        }
    }

    /// No Work command line ever turns a boundary off without asking.
    fn assert_nothing_bypassed(args: &[String]) {
        for arg in args {
            for bad in [
                "bypassPermissions",
                "dangerously",
                "danger-full-access",
                "acceptEdits",
            ] {
                assert!(!arg.contains(bad), "{bad} in {arg}");
            }
        }
    }

    fn settings_of(args: &[String]) -> serde_json::Value {
        let settings = args
            .windows(2)
            .find(|w| w[0] == "--settings")
            .map(|w| w[1].clone())
            .unwrap();
        serde_json::from_str(&settings).unwrap()
    }

    #[test]
    fn a_claude_work_run_asks_through_its_host_at_the_sandbox_s_edge() {
        let real = std::fs::canonicalize("/tmp").unwrap();
        let root = real.to_string_lossy().to_string();
        for grant in [WorkGrant::default(), full_grant()] {
            for resume in [None, Some("sess-1")] {
                let spec = work(Backend::ClaudeCode, grant.clone(), resume);
                let args = spec.args();
                assert_nothing_bypassed(&args);
                assert!(args
                    .windows(2)
                    .any(|w| w == ["--input-format", "stream-json"]));
                assert!(args
                    .windows(2)
                    .any(|w| w == ["--permission-prompt-tool", "stdio"]));
                assert!(
                    !args.contains(&"do it".to_string()),
                    "the prompt goes on stdin"
                );
                assert!(!args.contains(&"--".to_string()));
                assert!(matches!(spec.conversation(), Conversation::Claude(_)));
                let allowed = args.iter().position(|a| a == "--allowedTools").unwrap();
                let denied = args.iter().position(|a| a == "--disallowedTools").unwrap();
                let allowed = &args[allowed + 1..denied];
                let denied = &args[denied + 1..];
                assert!(
                    !allowed.contains(&"Bash".to_string()),
                    "Bash listed would skip asking: {allowed:?}"
                );
                assert!(
                    allowed.contains(&format!("Edit(/{root}/**)")),
                    "{allowed:?}"
                );
                for tool in ["WebFetch", "WebSearch"] {
                    assert!(denied.contains(&tool.to_string()), "{tool}: {denied:?}");
                }
                for name in [".git", ".claude", ".codex", ".mcp.json", "**/.git"] {
                    assert!(
                        denied.contains(&format!("Write(/{root}/{name}/**)")),
                        "{name}: {denied:?}"
                    );
                }
                let sandbox = &settings_of(&args)["sandbox"];
                let deny_write = sandbox["filesystem"]["denyWrite"].as_array().unwrap();
                assert!(
                    deny_write.contains(&serde_json::json!(format!("{root}/*/**/.git"))),
                    "{deny_write:?}"
                );
                assert_eq!(sandbox["enabled"], true);
                assert_eq!(sandbox["autoAllowBashIfSandboxed"], true);
                assert_eq!(
                    sandbox["allowUnsandboxedCommands"], true,
                    "it may ask to leave"
                );
                assert_eq!(sandbox["failIfUnavailable"], true);
                assert!(
                    sandbox["network"].get("strictAllowlist").is_none(),
                    "an unlisted host asks"
                );
                assert_eq!(
                    sandbox["network"]["allowedDomains"],
                    serde_json::json!(grant.network)
                );
                let allow_write = sandbox["filesystem"]["allowWrite"].as_array().unwrap();
                assert_eq!(allow_write[0], root.as_str(), "the worktree first");
                assert!(args.windows(2).any(|w| w == ["--setting-sources", "user"]));
                assert_eq!(
                    args.windows(2).any(|w| w[0] == "--resume"),
                    resume.is_some()
                );
            }
        }
        let ask = RunSpec {
            mode: Mode::Ask,
            ..work(Backend::ClaudeCode, WorkGrant::default(), None)
        };
        assert!(
            matches!(ask.conversation(), Conversation::Claude(_)),
            "a question can be steered too"
        );
        assert!(!ask.args().contains(&"--permission-prompt-tool".to_string()));
    }

    #[test]
    fn the_automatic_reviewer_is_each_cli_s_own() {
        let auto = WorkGrant {
            auto_review: true,
            ..WorkGrant::default()
        };
        let claude = work(Backend::ClaudeCode, auto.clone(), None).args();
        assert!(claude
            .windows(2)
            .any(|w| w == ["--permission-mode", "auto"]));
        assert!(!claude.iter().any(|a| a.contains("bypassPermissions")));
        let asking = work(Backend::ClaudeCode, WorkGrant::default(), None).args();
        assert!(asking
            .windows(2)
            .any(|w| w == ["--permission-mode", "default"]));
        let ask = RunSpec {
            mode: Mode::Ask,
            ..work(Backend::ClaudeCode, WorkGrant::default(), None)
        }
        .args();
        assert!(
            ask.windows(2)
                .any(|w| w == ["--permission-mode", "default"]),
            "a question never is"
        );

        let Conversation::Codex(server) = work(Backend::Codex, auto, None).conversation() else {
            unreachable!()
        };
        assert_eq!(
            server_thread(&server).reviewer.as_deref(),
            Some("auto_review")
        );
        let Conversation::Codex(server) =
            work(Backend::Codex, WorkGrant::default(), None).conversation()
        else {
            unreachable!()
        };
        assert_eq!(server_thread(&server).reviewer.as_deref(), Some("user"));
    }

    #[test]
    fn trust_reads_the_repository_s_settings_and_mcp_config() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        std::fs::write(real.join(".mcp.json"), "{}").unwrap();
        let spec = RunSpec {
            cwd: real.clone(),
            ..work(
                Backend::ClaudeCode,
                WorkGrant {
                    trust: true,
                    ..WorkGrant::default()
                },
                None,
            )
        };
        let args = spec.args();
        assert!(args
            .windows(2)
            .any(|w| w == ["--setting-sources", "user,project,local"]));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--mcp-config" && w[1] == real.join(".mcp.json").to_string_lossy()));
    }

    #[test]
    fn a_codex_work_run_is_an_app_server_thread_with_the_work_profile() {
        let real = std::fs::canonicalize("/tmp").unwrap();
        let root = real.to_string_lossy().to_string();
        let spec = work(Backend::Codex, full_grant(), None);
        let args = spec.args();
        assert_nothing_bypassed(&args);
        assert_eq!(
            &args[..7],
            [
                "app-server",
                "--disable",
                "plugins",
                "--disable",
                "hooks",
                "--disable",
                "apps"
            ]
        );
        let Conversation::Codex(server) = spec.conversation() else {
            panic!("a Codex Work run talks to an app-server");
        };
        let config = &server_thread(&server).config;
        assert_eq!(config["default_permissions"], "perch_work");
        let filesystem = &config["permissions"]["perch_work"]["filesystem"];
        assert_eq!(filesystem[":root"], "read");
        assert_eq!(filesystem[":workspace_roots"], "write");
        assert_eq!(filesystem["/opt/cache"], "write");
        assert_eq!(filesystem[format!("{root}/.git")], "read");
        assert_eq!(filesystem[format!("{root}/.claude")], "read");
        assert_eq!(filesystem[format!("{root}/*/**/.git")], "deny");
        assert!(filesystem.get("network").is_none());
        assert_eq!(
            config["projects"][root.as_str()]["trust_level"],
            "untrusted"
        );

        let trusted = work(
            Backend::Codex,
            WorkGrant {
                trust: true,
                ..WorkGrant::default()
            },
            Some("th-1"),
        );
        let Conversation::Codex(server) = trusted.conversation() else {
            unreachable!()
        };
        assert_eq!(
            server_thread(&server).config["projects"][root.as_str()]["trust_level"],
            "trusted"
        );
        assert_eq!(server_thread(&server).resume.as_deref(), Some("th-1"));
    }

    #[test]
    fn a_codex_question_is_a_read_only_app_server_thread_that_never_asks() {
        let real = std::fs::canonicalize("/tmp").unwrap();
        let root = real.to_string_lossy().to_string();
        for resume in [None, Some("th-1")] {
            let spec = RunSpec {
                mode: Mode::Ask,
                ..work(Backend::Codex, full_grant(), resume)
            };
            let args = spec.args();
            assert_nothing_bypassed(&args);
            assert_eq!(
                &args[..7],
                [
                    "app-server",
                    "--disable",
                    "plugins",
                    "--disable",
                    "hooks",
                    "--disable",
                    "apps"
                ]
            );
            let Conversation::Codex(server) = spec.conversation() else {
                unreachable!()
            };
            let thread = server_thread(&server);
            assert_eq!(thread.sandbox.as_deref(), Some("read-only"));
            assert_eq!(thread.approval_policy, "never");
            assert_eq!(thread.reviewer, None);
            assert_eq!(thread.resume.as_deref(), resume);
            assert!(
                thread.config.get("default_permissions").is_none(),
                "no Work profile"
            );
            assert!(thread.config.get("permissions").is_none());
            assert_eq!(
                thread.config["projects"][root.as_str()]["trust_level"],
                "untrusted",
                "never trusted"
            );
            assert_eq!(thread.config["model_reasoning_effort"], "high");
        }
    }

    #[test]
    fn codex_work_denies_a_git_below_the_worktree_s_top_and_reads_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        let common = real.join("repo/.git");
        let own = common.join("worktrees/wt");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("commondir"), "../..\n").unwrap();
        let worktree = real.join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", own.display()),
        )
        .unwrap();

        for grant in [WorkGrant::default(), full_grant()] {
            let entries = codex_work_entries(&worktree, &grant);
            let access = |path: &std::path::Path| {
                entries
                    .iter()
                    .find(|(p, _)| *p == path.to_string_lossy())
                    .map(|(_, a)| *a)
            };
            assert_eq!(
                access(&worktree.join("*/**/.git")),
                Some("deny"),
                "{entries:?}"
            );
            assert_eq!(access(&worktree.join(".git")), Some("read"), "{entries:?}");
            assert_eq!(access(&own), Some("read"), "{entries:?}");
            assert_eq!(access(&common), Some("read"), "{entries:?}");
            // Codex's `**/` matches no directory too, and a deny there
            // would keep git from reading the worktree's own `.git`.
            let denied: Vec<_> = entries.iter().filter(|(_, a)| *a == "deny").collect();
            assert_eq!(denied.len(), 1, "{denied:?}");
            assert!(
                denied.iter().all(|(p, _)| p.ends_with("/*/**/.git")),
                "{denied:?}"
            );
        }
    }

    fn server_thread(server: &interactive::CodexServer) -> interactive::CodexThread {
        server.thread_for_tests()
    }

    #[test]
    fn the_commit_grant_opens_the_git_directories_but_not_their_config_or_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        let common = real.join("repo/.git");
        let own = common.join("worktrees/wt");
        let other = common.join("worktrees/other");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(own.join("commondir"), "../..\n").unwrap();
        let worktree = real.join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", own.display()),
        )
        .unwrap();

        let grant = WorkGrant {
            commit: true,
            ..WorkGrant::default()
        };
        let settings: serde_json::Value =
            serde_json::from_str(&claude_work_settings(&worktree, &grant)).unwrap();
        let allow = settings["sandbox"]["filesystem"]["allowWrite"]
            .as_array()
            .unwrap();
        let deny = settings["sandbox"]["filesystem"]["denyWrite"]
            .as_array()
            .unwrap();
        let has = |list: &Vec<serde_json::Value>, path: &std::path::Path| {
            list.contains(&serde_json::json!(path.to_string_lossy()))
        };
        assert!(has(allow, &common) && has(allow, &own), "{allow:?}");
        assert!(!has(deny, &common) && !has(deny, &own), "{deny:?}");
        for guarded in [
            common.join("config"),
            common.join("hooks"),
            own.join("config.worktree"),
            own.join("commondir"),
        ] {
            assert!(has(deny, &guarded), "{} in {deny:?}", guarded.display());
        }
        assert!(
            has(deny, &worktree.join(".git")),
            "the pointer file itself stays read-only"
        );

        // What git uses elsewhere: the other worktrees, the ones added
        // later, the submodules, and a `.git` the run makes below its own.
        for guarded in [
            other.clone(),
            common.join("worktrees/*/commondir"),
            common.join("worktrees/*/config.worktree"),
            common.join("modules"),
            worktree.join("*/**/.git"),
        ] {
            assert!(has(deny, &guarded), "{} in {deny:?}", guarded.display());
        }
        assert!(
            !has(deny, &common.join("worktrees")),
            "denying it would deny the run's own: {deny:?}"
        );
    }

    #[test]
    fn the_commit_grant_opens_codex_s_git_directories_but_not_their_config_or_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        let common = real.join("repo/.git");
        let own = common.join("worktrees/wt");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("commondir"), "../..\n").unwrap();
        let worktree = real.join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", own.display()),
        )
        .unwrap();

        let grant = WorkGrant {
            commit: true,
            ..WorkGrant::default()
        };
        let entries = codex_work_entries(&worktree, &grant);
        let access = |path: &std::path::Path| {
            entries
                .iter()
                .find(|(p, _)| *p == path.to_string_lossy())
                .map(|(_, a)| *a)
        };
        assert_eq!(access(&common), Some("write"), "{entries:?}");
        assert_eq!(access(&own), Some("write"), "{entries:?}");
        // The run's own directory is more specific than `worktrees/`, so
        // it stays writable while every other worktree's, now or later,
        // does not.
        for guarded in [
            common.join("config"),
            common.join("hooks"),
            common.join("worktrees"),
            common.join("modules"),
            own.join("config.worktree"),
            own.join("commondir"),
            worktree.join(".git"),
        ] {
            assert_eq!(
                access(&guarded),
                Some("read"),
                "{} in {entries:?}",
                guarded.display()
            );
        }
        let denied: Vec<_> = entries.iter().filter(|(_, a)| *a == "deny").collect();
        assert_eq!(denied.len(), 1, "a glob deny stops reads too: {denied:?}");

        let main = real.join("main");
        let git = main.join(".git");
        std::fs::create_dir_all(git.join("worktrees/elsewhere")).unwrap();
        let entries = codex_work_entries(&main, &grant);
        let access = |path: &std::path::Path| {
            entries
                .iter()
                .find(|(p, _)| *p == path.to_string_lossy())
                .map(|(_, a)| *a)
        };
        assert_eq!(access(&git), Some("write"), "{entries:?}");
        for guarded in [
            git.join("config"),
            git.join("hooks"),
            git.join("worktrees"),
            git.join("modules"),
        ] {
            assert_eq!(
                access(&guarded),
                Some("read"),
                "{} in {entries:?}",
                guarded.display()
            );
        }
    }

    #[test]
    fn the_commit_grant_in_a_main_checkout_keeps_its_git_directory_open() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = dir.path().canonicalize().unwrap();
        let git = worktree.join(".git");
        let linked = git.join("worktrees/elsewhere");
        std::fs::create_dir_all(&linked).unwrap();

        let grant = WorkGrant {
            commit: true,
            ..WorkGrant::default()
        };
        let settings: serde_json::Value =
            serde_json::from_str(&claude_work_settings(&worktree, &grant)).unwrap();
        let allow = settings["sandbox"]["filesystem"]["allowWrite"]
            .as_array()
            .unwrap();
        let deny = settings["sandbox"]["filesystem"]["denyWrite"]
            .as_array()
            .unwrap();
        let has = |list: &Vec<serde_json::Value>, path: &std::path::Path| {
            list.contains(&serde_json::json!(path.to_string_lossy()))
        };
        assert!(has(allow, &git) && !has(deny, &git), "{allow:?} {deny:?}");
        for guarded in [
            git.join("config"),
            git.join("hooks"),
            linked,
            git.join("modules"),
            worktree.join("*/**/.git"),
        ] {
            assert!(has(deny, &guarded), "{} in {deny:?}", guarded.display());
        }
        assert!(
            !deny.iter().any(|path| path
                .as_str()
                .is_some_and(|p| p.ends_with("/**/.git") && !p.ends_with("/*/**/.git"))),
            "`**/.git` would match the top-level `.git` too: {deny:?}"
        );
    }

    #[test]
    fn a_linked_worktree_s_git_directories_are_denied_to_work() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        let common = real.join("repo/.git");
        let own = common.join("worktrees/wt");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("commondir"), "../..\n").unwrap();
        let worktree = real.join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", own.display()),
        )
        .unwrap();

        assert_eq!(linked_git_dirs(&worktree), [own.clone(), common.clone()]);
        let settings: serde_json::Value =
            serde_json::from_str(&claude_work_settings(&worktree, &WorkGrant::default())).unwrap();
        let denied = settings["sandbox"]["filesystem"]["denyWrite"]
            .as_array()
            .unwrap();
        assert!(
            denied.contains(&serde_json::json!(common.to_string_lossy())),
            "{denied:?}"
        );
        assert!(
            denied.contains(&serde_json::json!(own.to_string_lossy())),
            "{denied:?}"
        );
        assert!(
            linked_git_dirs(&real).is_empty(),
            "no .git file, nothing linked"
        );
    }

    #[test]
    fn the_sandbox_names_the_real_worktree_path() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&claude_settings(&link)).unwrap();
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyWrite"],
            serde_json::json!([real.to_string_lossy()])
        );
        let missing = dir.path().join("gone");
        let settings: serde_json::Value = serde_json::from_str(&claude_settings(&missing)).unwrap();
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyWrite"],
            serde_json::json!([missing.to_string_lossy()]),
            "an unresolvable path is passed as given"
        );
    }

    #[test]
    fn secrets_are_scrubbed_by_name_and_the_agents_own_keys_are_not() {
        for name in [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "PERCH_GIT_TOKEN",
            "NPM_TOKEN",
            "AWS_SECRET",
            "AWS_SECRET_ACCESS_KEY",
            "STRIPE_KEY",
            "github_token",
            "some_api_key",
            "OPENROUTER_APIKEY",
            "DOCKER_PASSWORD",
            "DB_PASSWD",
            "SMTP_PASS",
            "GITHUB_PAT",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "AZURE_CREDENTIAL",
        ] {
            assert!(is_scrubbed_env(name), "{name}");
        }
        for name in [
            "PATH",
            "HOME",
            "TOKENIZER",
            "KEYBOARD",
            "SECRETARY",
            "COMPASS",
            "PATTERN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
        ] {
            assert!(!is_scrubbed_env(name), "{name}");
        }
    }

    #[test]
    fn efforts_round_trip_by_name() {
        for e in Effort::ALL {
            assert_eq!(Effort::parse(e.name()), Some(e));
        }
        assert_eq!(Effort::parse("XHIGH"), Some(Effort::XHigh));
        assert_eq!(Effort::parse("ultra"), Some(Effort::Ultra));
        assert_eq!(Effort::ALL.last(), Some(&Effort::Ultra));
        assert_eq!(Effort::Ultra.label(), "Ultra");
        assert_eq!(Effort::parse("turbo"), None);
    }

    #[test]
    fn backends_round_trip_by_name() {
        for b in Backend::ALL {
            assert_eq!(Backend::parse(b.name()), Some(b));
        }
        assert_eq!(Backend::parse("codex"), Some(Backend::Codex));
        assert_eq!(Backend::parse("Claude-Code"), Some(Backend::ClaudeCode));
        assert_eq!(Backend::parse("gemini"), None);
    }

    fn claude(effort: Effort) -> RunSpec {
        RunSpec {
            backend: Backend::ClaudeCode,
            binary: "claude".into(),
            model: "opus".into(),
            effort,
            cwd: "/tmp".into(),
            prompt: "why?".into(),
            system: None,
            resume: None,
            mode: Mode::Ask,
        }
    }

    #[test]
    fn ultra_is_ultracode_with_the_orchestration_tools_allowed() {
        let args = claude(Effort::Ultra).args();
        assert!(args.windows(2).any(|w| w == ["--effort", "ultracode"]));
        let allowed = args.iter().position(|a| a == "--allowedTools").unwrap();
        let denied = args.iter().position(|a| a == "--disallowedTools").unwrap();
        for tool in ORCHESTRATION_TOOLS {
            let at = args
                .iter()
                .position(|a| a == tool)
                .unwrap_or_else(|| panic!("{tool} allowed"));
            assert!(allowed < at && at < denied, "{tool} is in the allowed list");
        }
        assert!(
            args.contains(&"Write".to_string()),
            "subagents inherit the denial"
        );
        assert!(!args
            .iter()
            .any(|a| a.contains("dangerously") || a.contains("bypass")));
    }

    #[test]
    fn max_stays_max_without_the_orchestration_tools() {
        let args = claude(Effort::Max).args();
        assert!(args.windows(2).any(|w| w == ["--effort", "max"]));
        assert!(!args.iter().any(|a| a == "ultracode"));
        for tool in ORCHESTRATION_TOOLS {
            assert!(
                !args.contains(&tool.to_string()),
                "{tool} is not allowed below ultra"
            );
        }
    }

    fn codex(resume: Option<&str>) -> RunSpec {
        RunSpec {
            backend: Backend::Codex,
            binary: "codex".into(),
            model: "gpt-5.6-sol".into(),
            effort: Effort::High,
            cwd: "/tmp/w t".into(),
            prompt: "why?".into(),
            system: Some("be \"brief\"\nplease".into()),
            resume: resume.map(str::to_string),
            mode: Mode::Ask,
        }
    }

    #[test]
    fn codex_efforts_pass_by_their_own_name() {
        let mut spec = codex(None);
        spec.effort = Effort::Ultra;
        let Conversation::Codex(server) = spec.conversation() else {
            unreachable!()
        };
        let thread = server_thread(&server);
        assert_eq!(thread.effort, "ultra");
        assert_eq!(thread.config["model_reasoning_effort"], "ultra");
        assert!(!spec.args().iter().any(|a| a.contains("ultracode")));
    }

    #[test]
    fn a_question_that_starts_with_a_dash_is_never_an_option() {
        for backend in Backend::ALL {
            let mut spec = claude(Effort::Low);
            spec.backend = backend;
            spec.prompt = "-1 comes back here. Why?".into();
            assert!(
                !spec.args().contains(&spec.prompt),
                "{}: the prompt goes on stdin",
                backend.name()
            );
        }
    }

    #[test]
    fn a_recovered_claude_permission_denial_does_not_type_a_later_failure() {
        let denied = r#"{"type":"system","subtype":"permission_denied","tool_name":"Write"}"#;
        let rejected_tool = r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":"Claude requested permissions","tool_use_id":"write-1"}]}}"#;
        let resumed = r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"text","text":"I will use another approach."}]}}"#;
        let failed = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"The service became unavailable"}"#;

        let mut parser = Parser::for_backend(Backend::ClaudeCode);
        parser.parse_line(denied).unwrap();
        parser.parse_line(rejected_tool).unwrap();
        parser.parse_line(resumed).unwrap();
        assert!(matches!(
            parser.parse_line(failed).unwrap().as_slice(),
            [RunEvent::Failed { message }] if message == "The service became unavailable"
        ));
        assert!(!parser.permission_denied());

        let mut unrecovered = Parser::for_backend(Backend::ClaudeCode);
        unrecovered.parse_line(denied).unwrap();
        unrecovered.parse_line(failed).unwrap();
        assert!(unrecovered.permission_denied());
    }

    #[test]
    fn toml_strings_escape_what_toml_requires() {
        assert_eq!(toml_string("plain"), r#""plain""#);
        assert_eq!(toml_string(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(toml_string(r"C:\x"), r#""C:\\x""#);
        assert_eq!(toml_string("a\nb"), r#""a\nb""#);
        assert_eq!(toml_string("a\tb"), r#""a\tb""#);
        assert_eq!(toml_string("a\r\n\u{7}"), r#""a\r\n\u0007""#);
        assert_eq!(toml_string(""), r#""""#);
    }

    #[test]
    fn prompt_paths_name_the_home_as_a_tilde() {
        let home = std::path::Path::new("/Users/kumarujjawal");
        let named = |path: &str, home| prompt_path_in(std::path::Path::new(path), home);
        assert_eq!(
            named("/Users/kumarujjawal/.perch/context/review-1", Some(home)),
            "~/.perch/context/review-1"
        );
        assert_eq!(named("/Users/kumarujjawal", Some(home)), "~");
        assert_eq!(
            named("/Users/kumarujjawalx/.perch", Some(home)),
            "/Users/kumarujjawalx/.perch"
        );
        assert_eq!(
            named("/srv/perch/context/r1", Some(home)),
            "/srv/perch/context/r1"
        );
        assert_eq!(
            named("/Users/kumarujjawal/.perch", None),
            "/Users/kumarujjawal/.perch"
        );
        assert_eq!(
            named("/x/.perch", Some(std::path::Path::new(""))),
            "/x/.perch"
        );
    }
}
