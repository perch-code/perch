//! Run a coding agent as a child process and read what it does.
//!
//! Two agents: Claude Code, driven as `claude -p`, and Codex, driven as
//! `codex exec`, each with JSON events on stdout. No SDK and no library:
//! the binary on `PATH`, a child process, one JSON object per line. Perch
//! works with whatever version of the CLI the user has, and the parsers
//! are the whole contract with them.

mod codex;
mod discover;
mod parse;
mod review;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[cfg(unix)]
use std::os::fd::{FromRawFd, OwnedFd};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;
use tokio::sync::Notify;

pub use codex::CodexParser;
pub use discover::{
    codex_catalog, codex_default_model, find_binary, parse_codex_catalog, CodexModel,
};
pub use parse::{parse_line, summarize_tool_call};
pub use review::{
    assess_review_capability, detect_review_capability, extract_review, review_output_schema,
    CliVersion, EvidenceSpan, ReviewCapability, ReviewEffort, ReviewExtraction, ReviewFinding,
    ReviewLocation, ReviewManifest, ReviewManifestEntry, ReviewSeverity, ReviewSide, ReviewSpec,
    ReviewVerificationSpec, REVIEW_ADAPTER_VERSION, REVIEW_OUTPUT_FORMAT_VERSION,
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

/// The tools a read-only run may use. This list is the whole permission
/// story: in print mode anything not on it is denied without asking, and
/// the denial arrives as an event.
pub const READ_ONLY_TOOLS: &[&str] = &[
    "Read",
    "Glob",
    "Grep",
    "LS",
    "Bash(git diff:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git blame:*)",
    "Bash(rg:*)",
];

/// Tools that change files, named so that no setting of the user's can let
/// them through.
const WRITING_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Claude Code's delegation tools, allowed only at `Effort::Ultra`. The init
/// event lists the subagent tool as `Task` and the model calls it `Agent`,
/// so both are named. Subagents inherit the parent's allowed and denied
/// lists, which keeps them read-only too.
pub const ORCHESTRATION_TOOLS: &[&str] = &["Workflow", "Agent", "Task"];

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
    /// Claude Code only. Codex has no tool list; its sandbox is the
    /// permission story, and `args` sets it read-only.
    pub allowed_tools: Vec<String>,
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
        let mut args = vec![
            "-p".to_string(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--model".into(),
            self.model.clone(),
            "--effort".into(),
            self.effort.claude_name().into(),
            "--permission-mode".into(),
            "default".into(),
        ];
        if let Some(system) = &self.system {
            args.push("--append-system-prompt".into());
            args.push(system.clone());
        }
        if let Some(session) = &self.resume {
            args.push("--resume".into());
            args.push(session.clone());
        }
        args.push("--allowedTools".into());
        args.extend(self.allowed_tools.iter().cloned());
        if self.effort == Effort::Ultra {
            args.extend(ORCHESTRATION_TOOLS.iter().map(|t| t.to_string()));
        }
        args.push("--disallowedTools".into());
        args.extend(WRITING_TOOLS.iter().map(|t| t.to_string()));
        args.push("--".into());
        args.push(self.prompt.clone());
        args
    }

    /// `codex exec --json`. A new thread takes `-s` and `-C`; `resume`
    /// rejects both, so the sandbox goes in as config and the directory is
    /// the thread's own. The model is repeated on resume because Codex
    /// otherwise falls back to its config default.
    fn codex_args(&self) -> Vec<String> {
        let mut args = vec!["exec".to_string(), "--json".into()];
        match &self.resume {
            None => {
                args.extend(["-s".into(), "read-only".into()]);
                args.extend(["-C".into(), self.cwd.to_string_lossy().into_owned()]);
            }
            Some(thread) => {
                args.extend(["resume".into(), thread.clone()]);
            }
        }
        args.extend(["-m".into(), self.model.clone()]);
        if self.resume.is_some() {
            args.extend(["-c".into(), "sandbox_mode=\"read-only\"".into()]);
        }
        args.extend([
            "-c".into(),
            format!("model_reasoning_effort={}", self.effort.name()),
        ]);
        if let Some(system) = &self.system {
            args.extend([
                "-c".into(),
                format!("developer_instructions={}", toml_string(system)),
            ]);
        }
        args.push("--".into());
        args.push(self.prompt.clone());
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

/// One parser per backend. Claude associates an explicit permission denial
/// with the terminal result it caused; Codex remembers the thread and which
/// commands have begun.
enum Parser {
    Claude {
        pending_permission_denial: bool,
        terminal_permission_denial: Option<bool>,
    },
    Codex(CodexParser),
}

impl Parser {
    fn for_backend(backend: Backend) -> Self {
        match backend {
            Backend::ClaudeCode => Parser::Claude {
                pending_permission_denial: false,
                terminal_permission_denial: None,
            },
            Backend::Codex => Parser::Codex(CodexParser::new()),
        }
    }

    fn parse_line(&mut self, line: &str) -> Result<Vec<RunEvent>, String> {
        if line.trim().is_empty() {
            return Ok(Vec::new());
        }
        let value = serde_json::from_str::<serde_json::Value>(line)
            .map_err(|error| format!("The agent emitted malformed JSON: {error}"))?;
        Ok(match self {
            Parser::Claude {
                pending_permission_denial,
                terminal_permission_denial,
            } => {
                let events = parse_line(line);
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
            Parser::Codex(p) => p.parse_line(line),
        })
    }

    fn permission_denied(&self) -> bool {
        matches!(
            self,
            Parser::Claude {
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
    /// A structured denial reported by the CLI. It is activity rather than
    /// necessarily terminal: an agent may recover and complete after it.
    PermissionDenied {
        tool: Option<String>,
        message: String,
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
}

impl Run {
    /// Start the process and read its events onto `events` until it ends.
    ///
    /// Must be called on a tokio runtime. Exactly one of `Finished`,
    /// `Failed`, and `Stopped` is the last event sent.
    pub fn spawn(spec: RunSpec, events: UnboundedSender<RunEvent>) -> Result<Run, Error> {
        let args = spec.args();
        Self::spawn_process(spec.backend, spec.binary, spec.cwd, args, events)
    }

    /// Start an installed CLI's native local code-review operation using
    /// the same process-tree lifetime, cancellation, event parsing, and
    /// watchdog guarantees as an ordinary question.
    pub fn spawn_review(spec: ReviewSpec, events: UnboundedSender<RunEvent>) -> Result<Run, Error> {
        spec.validate().map_err(Error::InvalidReview)?;
        let args = spec.args();
        Self::spawn_process(spec.backend, spec.binary, spec.cwd, args, events)
    }

    /// Start a structured prior-finding verification pass without inheriting
    /// repository instructions, hooks, plugins, MCP servers, or approvals.
    pub fn spawn_review_verification(
        spec: ReviewVerificationSpec,
        events: UnboundedSender<RunEvent>,
    ) -> Result<Run, Error> {
        spec.validate().map_err(Error::InvalidReview)?;
        let args = spec.args();
        Self::spawn_process(spec.backend, spec.binary, spec.cwd, args, events)
    }

    fn spawn_process(
        backend: Backend,
        binary: PathBuf,
        cwd: PathBuf,
        args: Vec<String>,
        events: UnboundedSender<RunEvent>,
    ) -> Result<Run, Error> {
        let started_at = Instant::now();
        let mut command = Command::new(&binary);
        command
            .args(args)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
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
        let run = Run {
            cancel: cancel.clone(),
            outcome,
        };

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut stderr_task = tokio::spawn(read_stderr_tail(stderr));
            let mut parser = Parser::for_backend(backend);
            let exit = drive_until_exit(
                &mut child,
                process_group,
                &cancel,
                &mut lines,
                &mut parser,
                &events,
                started_at,
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

async fn drive_until_exit(
    child: &mut Child,
    process_group: Option<u32>,
    cancel: &Cancellation,
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    parser: &mut Parser,
    events: &UnboundedSender<RunEvent>,
    started_at: Instant,
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
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    if let Err(failure) = apply_line(parser, &line, events, started_at, &mut terminal) {
                        let exit = failure.into();
                        let _ = stop_and_reap(child, process_group, lines, parser, events, started_at, &mut terminal).await;
                        return exit;
                    }
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

async fn read_stderr_tail(stderr: tokio::process::ChildStderr) -> String {
    use std::collections::VecDeque;
    let mut tail = VecDeque::with_capacity(3);
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(mut line)) = lines.next_line().await {
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
            allowed_tools: READ_ONLY_TOOLS.iter().map(|t| t.to_string()).collect(),
        };
        let args = spec.args();
        assert_eq!(args[0], "-p");
        assert_eq!(&args[args.len() - 2..], ["--", "why?"]);
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
            allowed_tools: vec![],
        };
        let args = spec.args();
        assert!(args.windows(2).any(|w| w == ["--resume", "sess-1"]));
        assert!(!args.contains(&"--append-system-prompt".to_string()));
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
            allowed_tools: READ_ONLY_TOOLS.iter().map(|t| t.to_string()).collect(),
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
            allowed_tools: READ_ONLY_TOOLS.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn a_new_codex_run_is_sandboxed_in_its_directory() {
        let args = codex(None).args();
        assert_eq!(&args[..2], ["exec", "--json"]);
        assert!(args.windows(2).any(|w| w == ["-s", "read-only"]));
        assert!(args.windows(2).any(|w| w == ["-C", "/tmp/w t"]));
        assert!(args.windows(2).any(|w| w == ["-m", "gpt-5.6-sol"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["-c", "model_reasoning_effort=high"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["-c", "developer_instructions=\"be \\\"brief\\\"\\nplease\""]));
        assert_eq!(&args[args.len() - 2..], ["--", "why?"]);
        assert!(!args.contains(&"resume".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("sandbox_mode")));
        assert!(!args.iter().any(|a| a.contains("ephemeral")
            || a.contains("dangerously")
            || a.contains("approve")));
        assert!(
            !args.iter().any(|a| a.starts_with("Bash(")),
            "the tool list is Claude's"
        );
    }

    #[test]
    fn a_codex_resume_repeats_the_model_and_sandboxes_by_config() {
        let args = codex(Some("01a0b584-d485-7d52-84b0-231e5b9b82d4")).args();
        assert_eq!(
            &args[..4],
            [
                "exec",
                "--json",
                "resume",
                "01a0b584-d485-7d52-84b0-231e5b9b82d4"
            ]
        );
        assert!(args.windows(2).any(|w| w == ["-m", "gpt-5.6-sol"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["-c", "sandbox_mode=\"read-only\""]));
        assert!(args
            .windows(2)
            .any(|w| w == ["-c", "model_reasoning_effort=high"]));
        assert_eq!(&args[args.len() - 2..], ["--", "why?"]);
        assert!(!args.contains(&"-s".to_string()));
        assert!(!args.contains(&"-C".to_string()));
    }

    #[test]
    fn codex_efforts_pass_by_their_own_name() {
        let mut spec = codex(None);
        spec.effort = Effort::Ultra;
        assert!(spec
            .args()
            .windows(2)
            .any(|w| w == ["-c", "model_reasoning_effort=ultra"]));
        assert!(!spec.args().iter().any(|a| a.contains("ultracode")));
    }

    #[test]
    fn a_question_that_starts_with_a_dash_is_still_the_prompt() {
        for backend in Backend::ALL {
            let mut spec = claude(Effort::Low);
            spec.backend = backend;
            spec.prompt = "-1 comes back here. Why?".into();
            let args = spec.args();
            assert_eq!(
                &args[args.len() - 2..],
                ["--", "-1 comes back here. Why?"],
                "{}",
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
}
