//! Run a coding agent as a child process and read what it does.
//!
//! Two agents: Claude Code, driven as `claude -p`, and Codex, driven as
//! `codex exec`, each with JSON events on stdout. No SDK and no library:
//! the binary on `PATH`, a child process, one JSON object per line. Perch
//! works with whatever version of the CLI the user has, and the parsers
//! are the whole contract with them.

mod codex;
mod parse;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Notify;

pub use codex::CodexParser;
pub use parse::{parse_line, summarize_tool_call};

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
        Self::ALL.into_iter().find(|b| b.name().eq_ignore_ascii_case(s.trim()))
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
    pub const ALL: [Effort; 6] =
        [Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max, Effort::Ultra];

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
        Self::ALL.into_iter().find(|e| e.name().eq_ignore_ascii_case(s.trim()))
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
        args.extend(["-c".into(), format!("model_reasoning_effort={}", self.effort.name())]);
        if let Some(system) = &self.system {
            args.extend(["-c".into(), format!("developer_instructions={}", toml_string(system))]);
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

/// One parser per backend. Claude Code's is a pure function of the line;
/// Codex's remembers the thread and which commands have begun.
enum Parser {
    Claude,
    Codex(CodexParser),
}

impl Parser {
    fn for_backend(backend: Backend) -> Self {
        match backend {
            Backend::ClaudeCode => Parser::Claude,
            Backend::Codex => Parser::Codex(CodexParser::new()),
        }
    }

    fn parse_line(&mut self, line: &str) -> Vec<RunEvent> {
        match self {
            Parser::Claude => parse_line(line),
            Parser::Codex(p) => p.parse_line(line),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    Started { session_id: String, model: String },
    /// Something the agent said. One per message, not per token.
    Text(String),
    ToolCall { id: String, name: String, summary: String },
    ToolResult { id: String, ok: bool, summary: String },
    Finished { session_id: String, cost_usd: Option<f64>, duration_ms: u64 },
    Failed { message: String },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not start {binary}: {source}")]
    Spawn { binary: PathBuf, source: std::io::Error },
}

/// A running agent. Dropping it does not stop the process; `cancel` does.
#[derive(Debug, Clone)]
pub struct Run {
    cancel: Arc<Notify>,
}

impl Run {
    /// Start the process and read its events onto `events` until it ends.
    ///
    /// Must be called on a tokio runtime. Exactly one of `Finished` and
    /// `Failed` is the last event sent.
    pub fn spawn(spec: RunSpec, events: UnboundedSender<RunEvent>) -> Result<Run, Error> {
        let started_at = Instant::now();
        let mut child = Command::new(&spec.binary)
            .args(spec.args())
            .current_dir(&spec.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| Error::Spawn { binary: spec.binary.clone(), source })?;

        let cancel = Arc::new(Notify::new());
        let run = Run { cancel: cancel.clone() };

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let mut parser = Parser::for_backend(spec.backend);

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let stderr_task = tokio::spawn(async move {
                let mut text = String::new();
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    text.push_str(&line);
                    text.push('\n');
                }
                text
            });

            let mut ended = false;
            let mut stopped = false;
            loop {
                tokio::select! {
                    line = lines.next_line() => match line {
                        Ok(Some(line)) => {
                            for mut event in parser.parse_line(&line) {
                                // Codex's stream has no duration, so the
                                // wall clock since spawn stands in.
                                if let RunEvent::Finished { duration_ms: ms @ 0, .. } = &mut event {
                                    *ms = started_at.elapsed().as_millis() as u64;
                                }
                                ended |= matches!(event, RunEvent::Finished { .. } | RunEvent::Failed { .. });
                                if events.send(event).is_err() {
                                    return;
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!(error = %e, "agent stdout unreadable");
                            break;
                        }
                    },
                    _ = cancel.notified() => {
                        let _ = child.start_kill();
                        stopped = true;
                        break;
                    }
                }
            }

            let status = child.wait().await;
            // A killed shell can leave a grandchild holding stderr open, so
            // never wait on it for long; the tail is only for the message.
            let stderr = if stopped {
                stderr_task.abort();
                String::new()
            } else {
                tokio::time::timeout(std::time::Duration::from_secs(2), stderr_task)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default()
            };

            if ended {
                return;
            }
            let message = if stopped {
                "Stopped".to_string()
            } else {
                let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ");
                match status {
                    Ok(s) if s.success() => "The agent ended without answering".to_string(),
                    Ok(s) => format!("The agent exited with {s}{}", if tail.is_empty() { String::new() } else { format!(": {tail}") }),
                    Err(e) => format!("Could not wait for the agent: {e}"),
                }
            };
            let _ = events.send(RunEvent::Failed { message });
        });

        Ok(run)
    }

    /// Kill the process. A `Failed { "Stopped" }` follows on the channel.
    pub fn cancel(&self) {
        self.cancel.notify_one();
    }
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
        assert!(args.windows(2).any(|w| w == ["--append-system-prompt", "be brief"]));
        assert!(args.contains(&"Bash(git log:*)".to_string()));
        assert!(args.contains(&"Write".to_string()), "writing tools are named to be denied");
        assert!(!args.iter().any(|a| a.contains("dangerously") || a.contains("bypass")));
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
            let at = args.iter().position(|a| a == tool).unwrap_or_else(|| panic!("{tool} allowed"));
            assert!(allowed < at && at < denied, "{tool} is in the allowed list");
        }
        assert!(args.contains(&"Write".to_string()), "subagents inherit the denial");
        assert!(!args.iter().any(|a| a.contains("dangerously") || a.contains("bypass")));
    }

    #[test]
    fn max_stays_max_without_the_orchestration_tools() {
        let args = claude(Effort::Max).args();
        assert!(args.windows(2).any(|w| w == ["--effort", "max"]));
        assert!(!args.iter().any(|a| a == "ultracode"));
        for tool in ORCHESTRATION_TOOLS {
            assert!(!args.contains(&tool.to_string()), "{tool} is not allowed below ultra");
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
        assert!(args.windows(2).any(|w| w == ["-c", "model_reasoning_effort=high"]));
        assert!(args.windows(2).any(|w| w == ["-c", "developer_instructions=\"be \\\"brief\\\"\\nplease\""]));
        assert_eq!(&args[args.len() - 2..], ["--", "why?"]);
        assert!(!args.contains(&"resume".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("sandbox_mode")));
        assert!(!args.iter().any(|a| a.contains("ephemeral") || a.contains("dangerously") || a.contains("approve")));
        assert!(!args.iter().any(|a| a.starts_with("Bash(")), "the tool list is Claude's");
    }

    #[test]
    fn a_codex_resume_repeats_the_model_and_sandboxes_by_config() {
        let args = codex(Some("01a0b584-d485-7d52-84b0-231e5b9b82d4")).args();
        assert_eq!(&args[..4], ["exec", "--json", "resume", "01a0b584-d485-7d52-84b0-231e5b9b82d4"]);
        assert!(args.windows(2).any(|w| w == ["-m", "gpt-5.6-sol"]));
        assert!(args.windows(2).any(|w| w == ["-c", "sandbox_mode=\"read-only\""]));
        assert!(args.windows(2).any(|w| w == ["-c", "model_reasoning_effort=high"]));
        assert_eq!(&args[args.len() - 2..], ["--", "why?"]);
        assert!(!args.contains(&"-s".to_string()));
        assert!(!args.contains(&"-C".to_string()));
    }

    #[test]
    fn codex_efforts_pass_by_their_own_name() {
        let mut spec = codex(None);
        spec.effort = Effort::Ultra;
        assert!(spec.args().windows(2).any(|w| w == ["-c", "model_reasoning_effort=ultra"]));
        assert!(!spec.args().iter().any(|a| a.contains("ultracode")));
    }

    #[test]
    fn a_question_that_starts_with_a_dash_is_still_the_prompt() {
        for backend in Backend::ALL {
            let mut spec = claude(Effort::Low);
            spec.backend = backend;
            spec.prompt = "-1 comes back here. Why?".into();
            let args = spec.args();
            assert_eq!(&args[args.len() - 2..], ["--", "-1 comes back here. Why?"], "{}", backend.name());
        }
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
