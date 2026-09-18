//! Run a coding agent as a child process and read what it does.
//!
//! One agent for now, Claude Code, driven as `claude -p` with JSON events
//! on stdout. No SDK and no library: the binary on `PATH`, a child process,
//! one JSON object per line. Perch works with whatever version of the CLI
//! the user has, and the parser is the whole contract with it.

mod parse;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Notify;

pub use parse::{parse_line, summarize_tool_call};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    ClaudeCode,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::ClaudeCode => "claude-code",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effort {
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 5] = [Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max];

    pub fn name(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
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

#[derive(Debug, Clone)]
pub struct RunSpec {
    pub backend: Backend,
    /// The executable: `claude`, or a path from config.
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
    pub allowed_tools: Vec<String>,
}

impl RunSpec {
    /// The command line, for the log and for tests.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec![
            "-p".to_string(),
            self.prompt.clone(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--model".into(),
            self.model.clone(),
            "--effort".into(),
            self.effort.name().into(),
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
        args.push("--disallowedTools".into());
        args.extend(WRITING_TOOLS.iter().map(|t| t.to_string()));
        args
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
                            for event in parse_line(&line) {
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
        assert_eq!(&args[..2], ["-p", "why?"]);
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
        assert_eq!(Effort::parse("turbo"), None);
    }
}
