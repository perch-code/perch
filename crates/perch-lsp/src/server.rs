//! Finding and describing language servers.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A language server Perch knows how to run. A worktree runs at most one
/// of each kind, rooted at the worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ServerKind {
    RustAnalyzer,
}

impl ServerKind {
    pub const ALL: [ServerKind; 1] = [ServerKind::RustAnalyzer];

    /// The server's own name, as statuses and toasts say it.
    pub fn name(self) -> &'static str {
        match self {
            ServerKind::RustAnalyzer => "rust-analyzer",
        }
    }

    /// The kind that serves a file, by its extension. A path may be
    /// relative to the worktree or absolute.
    pub fn for_path(path: &str) -> Option<Self> {
        language_id(path).map(|(kind, _)| kind)
    }

    /// The kind that serves a fenced block's language, as a fence names
    /// it.
    pub fn for_fence(lang: &str) -> Option<Self> {
        match lang.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(ServerKind::RustAnalyzer),
            _ => None,
        }
    }

    /// The files at a worktree's root that say a project of this kind is
    /// there.
    pub fn markers(self) -> &'static [&'static str] {
        match self {
            ServerKind::RustAnalyzer => &["Cargo.toml"],
        }
    }

    /// How long the handshake waits for the server's answer.
    pub fn initialize_timeout(self) -> Duration {
        match self {
            ServerKind::RustAnalyzer => Duration::from_secs(5),
        }
    }

    /// Whether the server says on its own, with `$/progress`, when it is
    /// ready. One that doesn't is ready once it has answered the
    /// handshake.
    pub(crate) fn reports_readiness(self) -> bool {
        matches!(self, ServerKind::RustAnalyzer)
    }
}

/// The kind serving a file and the LSP language identifier its
/// `textDocument/didOpen` carries.
pub fn language_id(path: &str) -> Option<(ServerKind, &'static str)> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let (_, ext) = name.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "rs" => Some((ServerKind::RustAnalyzer, "rust")),
        _ => None,
    }
}

/// How to run a language server, and what it understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSpec {
    pub kind: ServerKind,
    pub binary: PathBuf,
    pub args: Vec<String>,
    /// Settings sent with `initialize`, in the server's own shape.
    pub options: serde_json::Value,
}

/// What the status line shows about a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No server binary for this language; the editor works without one.
    Absent,
    Starting,
    Indexing { percent: Option<u8> },
    Ready,
    /// Running, but it says it cannot do its work: rust-analyzer whose
    /// workspace failed to load answers every ask with nothing.
    Unhealthy { message: String },
    Failed { reason: String },
}

impl Status {
    /// The status as a line that names `kind`'s server.
    pub fn describe(&self, kind: ServerKind) -> String {
        let name = kind.name();
        match self {
            Status::Absent => format!("{name} not found"),
            Status::Starting => format!("{name} starting"),
            Status::Indexing { percent: Some(p) } => format!("{name} indexing {p}%"),
            Status::Indexing { percent: None } => format!("{name} indexing"),
            Status::Ready => format!("{name} ready"),
            // Its first line: rust-analyzer puts the tool output it failed
            // on under it, a usage text and all.
            Status::Unhealthy { message } => {
                format!("{name}: {}", message.lines().next().unwrap_or(message))
            }
            Status::Failed { reason } => format!("{name} failed: {reason}"),
        }
    }

    pub fn is_usable(&self) -> bool {
        matches!(self, Status::Indexing { .. } | Status::Ready)
    }
}

/// Locate a server of `kind` for the checkout at `root`.
///
/// For rust-analyzer: the one in the toolchain rustup picks for `root`
/// first, then PATH, then cargo's bin directory. A rust-analyzer supports
/// only toolchains from its own release back a few, so a newer one on PATH
/// refuses a checkout that pins an older toolchain; the one shipped with
/// that toolchain never does. Nothing is configurable: a review tool
/// should find the toolchain the user already has rather than ask them to
/// describe it.
pub fn find(kind: ServerKind, root: &Path) -> Option<ServerSpec> {
    match kind {
        ServerKind::RustAnalyzer => {
            let candidates = [
                rustup_rust_analyzer(root),
                which("rust-analyzer"),
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|h| h.join(".cargo/bin/rust-analyzer"))
                    .filter(|p| p.is_file()),
            ];
            candidates.into_iter().flatten().next().map(|binary| ServerSpec {
                kind,
                binary,
                args: Vec::new(),
                options: rust_analyzer_options(),
            })
        }
    }
}

/// The kinds whose project markers sit at `root`.
pub fn kinds_at_root(root: &Path) -> Vec<ServerKind> {
    ServerKind::ALL
        .into_iter()
        .filter(|kind| kind.markers().iter().any(|marker| root.join(marker).is_file()))
        .collect()
}

/// What rust-analyzer is told as it starts. Its workspace symbol search,
/// which looks up a name written in a comment, stops at the first 128
/// fuzzy matches in name order, which can leave out the one match that is
/// the name for a short one; and it lists every `pub use` of an item as an
/// item of its own, so a name re-exported twice would be three.
pub(crate) fn rust_analyzer_options() -> serde_json::Value {
    serde_json::json!({
        "workspace": {
            "symbol": { "search": { "limit": 1024, "excludeImports": true } }
        }
    })
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The rust-analyzer of the toolchain rustup would run in `root`, from its
/// rust-toolchain.toml or the default. Nothing when that toolchain or its
/// rust-analyzer component is not installed: finding a server should not
/// download one.
fn rustup_rust_analyzer(root: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("rustup")
        .args(["which", "rust-analyzer"])
        .current_dir(root)
        // As in `Client::start`: a toolchain pin inherited from `cargo run`
        // would outrank the checkout's own.
        .env_remove("RUSTUP_TOOLCHAIN")
        .env("RUSTUP_AUTO_INSTALL", "0")
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
    fn a_file_picks_its_server_by_extension() {
        assert_eq!(language_id("src/lib.rs"), Some((ServerKind::RustAnalyzer, "rust")));
        assert_eq!(language_id("/abs/MAIN.RS"), Some((ServerKind::RustAnalyzer, "rust")));
        assert_eq!(ServerKind::for_path("README.md"), None);
        assert_eq!(ServerKind::for_path("Makefile"), None);
        assert_eq!(ServerKind::for_fence("Rust"), Some(ServerKind::RustAnalyzer));
        assert_eq!(ServerKind::for_fence("diff"), None);
    }

    #[test]
    fn a_root_marker_names_its_kind() {
        let dir = std::env::temp_dir().join(format!("perch-lsp-markers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(kinds_at_root(&dir).is_empty());
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        assert_eq!(kinds_at_root(&dir), [ServerKind::RustAnalyzer]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_text_is_written_for_a_status_line() {
        let rust = ServerKind::RustAnalyzer;
        assert_eq!(Status::Ready.describe(rust), "rust-analyzer ready");
        assert_eq!(
            Status::Indexing { percent: Some(42) }.describe(rust),
            "rust-analyzer indexing 42%"
        );
        assert!(Status::Failed { reason: "boom".into() }.describe(rust).contains("boom"));
        assert_eq!(
            Status::Unhealthy {
                message: "Failed to load workspaces\n\nUsage: cargo metadata".into()
            }
            .describe(rust),
            "rust-analyzer: Failed to load workspaces"
        );
    }

    #[test]
    fn a_server_is_usable_while_it_is_still_indexing() {
        // Partial answers beat no answers; the editor does not wait.
        assert!(Status::Indexing { percent: Some(10) }.is_usable());
        assert!(Status::Ready.is_usable());
        assert!(!Status::Starting.is_usable());
        assert!(!Status::Absent.is_usable());
        assert!(!Status::Unhealthy { message: "no workspace".into() }.is_usable());
    }
}
