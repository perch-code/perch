//! Comment threads anchored to lines of a worktree, and the agent runs
//! that answer in them.
//!
//! Shaped like GitHub's review threads on purpose: `side` and `start_line`
//! are its names, so review threads fetched later can sit in the same list
//! with the same anchor type. Nothing here fetches or posts one; the
//! `origin` column exists so that milestone needs no migration.
//!
//! `CommentThread` rather than `Thread`: that word is the notification.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadOrigin {
    /// Perch's own. The truth; nothing replaces it.
    Local,
    /// A cache of GitHub's, replaceable wholesale by `github_id`.
    GitHub,
}

impl ThreadOrigin {
    pub fn name(self) -> &'static str {
        match self {
            ThreadOrigin::Local => "local",
            ThreadOrigin::GitHub => "github",
        }
    }

    fn parse(s: &str) -> Self {
        if s == "github" { ThreadOrigin::GitHub } else { ThreadOrigin::Local }
    }
}

/// Which file a line number counts in: the old one or the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }

    fn parse(s: &str) -> Self {
        if s == "left" { Side::Left } else { Side::Right }
    }
}

/// Lines a thread is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub path: String,
    /// Inclusive, one-based, in the file on `side`.
    pub start_line: u32,
    /// Inclusive; equal to `start_line` for one line.
    pub line: u32,
    pub side: Side,
    /// The head the lines were read at.
    pub commit_sha: String,
}

impl Anchor {
    /// `src/x.rs:41–58`, or `src/x.rs:41`.
    pub fn label(&self) -> String {
        if self.start_line == self.line {
            format!("{}:{}", self.path, self.line)
        } else {
            format!("{}:{}\u{2013}{}", self.path, self.start_line, self.line)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommentThread {
    /// Perch's own id for a local thread, GitHub's node id later.
    pub id: String,
    pub worktree_id: String,
    pub origin: ThreadOrigin,
    /// `None` for a thread about the worktree as a whole.
    pub anchor: Option<Anchor>,
    /// The file changed after the anchor was made; the lines may have moved.
    pub outdated: bool,
    pub resolved: bool,
    pub created_at: Timestamp,
    pub messages: Vec<CommentMessage>,
}

impl CommentThread {
    /// The session id of the last run that finished, which a follow-up
    /// resumes. `None` means start over with the history folded in.
    pub fn resumable_session(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .filter_map(|m| m.run.as_ref())
            .find(|r| r.status == RunStatus::Finished)
            .and_then(|r| r.session_id.clone())
    }

    pub fn running_run(&self) -> Option<&RunRecord> {
        self.messages
            .iter()
            .filter_map(|m| m.run.as_ref())
            .find(|r| r.status == RunStatus::Running)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Author {
    User,
    Agent { backend: String, model: String },
    GitHubUser(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommentMessage {
    pub id: String,
    pub thread_id: String,
    pub origin: ThreadOrigin,
    pub author: Author,
    pub body: String,
    pub created_at: Timestamp,
    /// An agent's answer carries the run that produced it. A user's
    /// question carries the run that is answering it while it runs, so
    /// the thread has somewhere to show the steps before there is an
    /// answer.
    pub run: Option<RunRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Finished,
    Failed,
    Stopped,
}

impl RunStatus {
    pub fn name(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Finished => "finished",
            RunStatus::Failed => "failed",
            RunStatus::Stopped => "stopped",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "finished" => RunStatus::Finished,
            "failed" => RunStatus::Failed,
            "stopped" => RunStatus::Stopped,
            _ => RunStatus::Running,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub id: String,
    pub thread_id: String,
    pub backend: String,
    pub model: String,
    pub effort: String,
    pub session_id: Option<String>,
    pub status: RunStatus,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    /// The tool lines, in order.
    pub steps: Vec<RunStep>,
    pub error: Option<String>,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStep {
    pub summary: String,
    pub ok: bool,
}

/// A fresh id: time-ordered, unique on this machine, safe in a URL.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = Timestamp::now().as_nanosecond();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}-{:x}", nanos, std::process::id(), n)
}

pub(crate) mod sql {
    //! The rows, so the cache module stays about SQL and not about parsing.

    use super::*;

    pub(crate) fn origin(s: &str) -> ThreadOrigin {
        ThreadOrigin::parse(s)
    }

    pub(crate) fn side(s: &str) -> Side {
        Side::parse(s)
    }

    pub(crate) fn status(s: &str) -> RunStatus {
        RunStatus::parse(s)
    }

    pub(crate) fn author(kind: &str, name: Option<String>, model: Option<String>) -> Author {
        match kind {
            "agent" => Author::Agent {
                backend: name.unwrap_or_default(),
                model: model.unwrap_or_default(),
            },
            "github_user" => Author::GitHubUser(name.unwrap_or_default()),
            _ => Author::User,
        }
    }

    pub(crate) fn author_columns(a: &Author) -> (&'static str, Option<String>) {
        match a {
            Author::User => ("user", None),
            Author::Agent { backend, .. } => ("agent", Some(backend.clone())),
            Author::GitHubUser(login) => ("github_user", Some(login.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_label_a_line_or_a_range() {
        let one = Anchor { path: "src/x.rs".into(), start_line: 41, line: 41, side: Side::Right, commit_sha: "h".into() };
        assert_eq!(one.label(), "src/x.rs:41");
        let range = Anchor { line: 58, ..one };
        assert_eq!(range.label(), "src/x.rs:41–58");
    }

    #[test]
    fn ids_are_unique_and_ordered() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert!(!a.contains('/'));
    }
}
