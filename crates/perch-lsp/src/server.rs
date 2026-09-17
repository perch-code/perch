//! Finding and describing language servers.

use std::path::PathBuf;

/// How to run a language server, and what it understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSpec {
    pub binary: PathBuf,
    pub args: Vec<String>,
    /// The LSP language identifier sent with `textDocument/didOpen`.
    pub language_id: &'static str,
}

/// What the status line shows about a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No server binary for this language; the editor works without one.
    Absent,
    Starting,
    Indexing { percent: Option<u8> },
    Ready,
    Failed { reason: String },
}

impl Status {
    pub fn describe(&self) -> String {
        match self {
            Status::Absent => "rust-analyzer not found".into(),
            Status::Starting => "rust-analyzer starting".into(),
            Status::Indexing { percent: Some(p) } => format!("rust-analyzer indexing {p}%"),
            Status::Indexing { percent: None } => "rust-analyzer indexing".into(),
            Status::Ready => "rust-analyzer ready".into(),
            Status::Failed { reason } => format!("rust-analyzer failed: {reason}"),
        }
    }

    pub fn is_usable(&self) -> bool {
        matches!(self, Status::Indexing { .. } | Status::Ready)
    }
}

/// Locate rust-analyzer.
///
/// PATH first, then cargo's bin directory, then whatever rustup would run.
/// Nothing is configurable: a review tool should find the toolchain the user
/// already has rather than ask them to describe it.
pub fn find_server(language: &str) -> Option<ServerSpec> {
    if language != "rust" {
        return None;
    }

    let candidates = [
        which("rust-analyzer"),
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join(".cargo/bin/rust-analyzer"))
            .filter(|p| p.is_file()),
        rustup_rust_analyzer(),
    ];

    candidates.into_iter().flatten().next().map(|binary| ServerSpec {
        binary,
        args: Vec::new(),
        language_id: "rust",
    })
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn rustup_rust_analyzer() -> Option<PathBuf> {
    let out = std::process::Command::new("rustup")
        .args(["which", "rust-analyzer"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_rust_has_a_server_configured() {
        assert!(find_server("go").is_none());
        assert!(find_server("python").is_none());
    }

    #[test]
    fn status_text_is_written_for_a_status_line() {
        assert_eq!(Status::Ready.describe(), "rust-analyzer ready");
        assert_eq!(
            Status::Indexing { percent: Some(42) }.describe(),
            "rust-analyzer indexing 42%"
        );
        assert!(Status::Failed { reason: "boom".into() }.describe().contains("boom"));
    }

    #[test]
    fn a_server_is_usable_while_it_is_still_indexing() {
        // Partial answers beat no answers; the editor does not wait.
        assert!(Status::Indexing { percent: Some(10) }.is_usable());
        assert!(Status::Ready.is_usable());
        assert!(!Status::Starting.is_usable());
        assert!(!Status::Absent.is_usable());
    }
}
