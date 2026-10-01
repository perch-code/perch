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
    /// No server for this language, and what it would take to have one;
    /// the editor works without it.
    Missing(Missing),
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
            Status::Missing(missing) => missing.describe(),
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

/// Where `find` looks, apart from the checkout: the `PATH` to search, the
/// home directory, and Perch's own install directory for servers. Taken
/// from the process by `Env::current`; tests give their own.
#[derive(Debug, Clone)]
pub struct Env {
    pub path: Option<std::ffi::OsString>,
    pub home: Option<PathBuf>,
    /// `~/.perch/servers`: one directory per server, one per version
    /// under it.
    pub servers: PathBuf,
}

impl Env {
    /// The process's `PATH` and home, with Perch's servers under
    /// `servers`.
    pub fn current(servers: PathBuf) -> Self {
        Env {
            path: std::env::var_os("PATH"),
            home: std::env::var_os("HOME").map(PathBuf::from),
            servers,
        }
    }

    /// The first `name` on `PATH` that is a file.
    pub fn which(&self, name: &str) -> Option<PathBuf> {
        self.which_all(name).next()
    }

    /// Every `name` on `PATH` that is a file, in `PATH`'s order.
    pub fn which_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = PathBuf> + 'a {
        self.path
            .iter()
            .flat_map(std::env::split_paths)
            .map(move |dir| dir.join(name))
            .filter(|candidate| candidate.is_file())
    }
}

/// What a server needs that isn't there, so Perch can't install it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Rustup,
}

/// A server that wasn't found: what's missing, and how to install it when
/// Perch can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub kind: ServerKind,
    /// What has to be installed first, by the user. No `install` then.
    pub needs: Option<Need>,
    pub install: Option<Box<Install>>,
    /// The rustup toolchain the checkout pins, for rust-analyzer.
    pub toolchain: Option<String>,
}

impl Missing {
    /// The line a toast shows.
    pub fn describe(&self) -> String {
        let name = self.kind.name();
        match (self.needs, &self.toolchain) {
            (Some(Need::Rustup), _) => format!("{name} needs rustup, which isn't on your PATH."),
            (None, Some(toolchain)) => {
                format!("{name} isn't installed for Rust {}.", short_toolchain(toolchain))
            }
            (None, None) => format!("{name} isn't installed."),
        }
    }
}

/// `1.98.1-aarch64-apple-darwin` as `1.98.1`: the toolchain without the
/// host it's for.
fn short_toolchain(toolchain: &str) -> &str {
    ["-aarch64-", "-x86_64-", "-arm64-", "-i686-"]
        .iter()
        .filter_map(|host| toolchain.find(host))
        .min()
        .map_or(toolchain, |at| &toolchain[..at])
}

/// How Perch installs a missing server. A command runs as it is shown,
/// word for word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// Run `program` with `args` and `env`. With `into`, the command
    /// writes to `into`'s `.partial` sibling, which is renamed to `into`
    /// when it succeeds; without, it installs somewhere of its own, as
    /// rustup does.
    Command {
        program: PathBuf,
        args: Vec<String>,
        env: Vec<(String, String)>,
        into: Option<PathBuf>,
    },
}

impl Install {
    /// The command as the toast's tooltip shows it.
    pub fn shown(&self) -> String {
        match self {
            Install::Command { program, args, env, .. } => {
                let program = program
                    .file_name()
                    .map_or_else(|| program.display().to_string(), |n| n.to_string_lossy().into_owned());
                env.iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .chain(std::iter::once(program))
                    .chain(args.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        }
    }
}

/// The `.partial` sibling an install into `into` writes to first.
pub fn partial(into: &Path) -> PathBuf {
    let name = into.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    into.with_file_name(format!("{name}.partial"))
}

/// Locate a server of `kind` for the checkout at `root`, or say what's
/// missing. Finding never downloads or installs anything.
///
/// For rust-analyzer: the one in the toolchain rustup picks for `root`.
/// A rust-analyzer supports only toolchains from its own release back a
/// few, so one from another release refuses a checkout that pins a newer
/// toolchain, with a `Failed to load workspaces` that says nothing of the
/// fix; the one shipped with that toolchain never does. So when rustup
/// manages the checkout and its toolchain has no rust-analyzer, the server
/// is missing, with the rustup command that adds it. Without rustup, the
/// first `rust-analyzer` on `PATH`. rustup's proxy is never a candidate:
/// it only ever runs the toolchain's rust-analyzer, or exits. Nothing is
/// configurable: a review tool should find the toolchain the user already
/// has rather than ask them to describe it.
pub fn find(kind: ServerKind, root: &Path, env: &Env) -> Result<ServerSpec, Missing> {
    match kind {
        ServerKind::RustAnalyzer => find_rust_analyzer(root, env),
    }
}

fn find_rust_analyzer(root: &Path, env: &Env) -> Result<ServerSpec, Missing> {
    let kind = ServerKind::RustAnalyzer;
    let spec = |binary| ServerSpec {
        kind,
        binary,
        args: Vec::new(),
        options: rust_analyzer_options(),
    };
    let rustup = env.which("rustup").or_else(|| {
        env.home
            .as_ref()
            .map(|home| home.join(".cargo/bin/rustup"))
            .filter(|p| p.is_file())
    });
    if let Some(rustup) = &rustup {
        let missing = |toolchain: String, args: &[&str]| Missing {
            kind,
            needs: None,
            install: Some(Box::new(Install::Command {
                program: rustup.clone(),
                args: args.iter().map(|a| a.replace("<t>", &toolchain)).collect(),
                env: Vec::new(),
                into: None,
            })),
            toolchain: Some(toolchain),
        };
        match rustup_which(rustup, root) {
            RustupWhich::Found(binary) => return Ok(spec(binary)),
            RustupWhich::NoComponent(toolchain) => {
                return Err(missing(
                    toolchain,
                    &["component", "add", "rust-analyzer", "--toolchain", "<t>"],
                ))
            }
            RustupWhich::NoToolchain(toolchain) => {
                return Err(missing(
                    toolchain,
                    &[
                        "toolchain",
                        "install",
                        "<t>",
                        "--profile",
                        "minimal",
                        "--component",
                        "rust-analyzer",
                    ],
                ))
            }
            RustupWhich::Other => {}
        }
    }
    let cargo_bin = env
        .home
        .as_ref()
        .map(|home| home.join(".cargo/bin/rust-analyzer"))
        .filter(|p| p.is_file());
    env.which_all("rust-analyzer")
        .chain(cargo_bin)
        .find(|candidate| !is_rustup_proxy(candidate, rustup.as_deref()))
        .map(spec)
        .ok_or(Missing {
            kind,
            needs: rustup.is_none().then_some(Need::Rustup),
            install: None,
            toolchain: None,
        })
}

/// What `rustup which rust-analyzer` said in a checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RustupWhich {
    Found(PathBuf),
    /// The toolchain is installed without the component.
    NoComponent(String),
    /// The toolchain itself is missing.
    NoToolchain(String),
    /// Anything else: treated as no rustup.
    Other,
}

/// Ask rustup for the rust-analyzer of the toolchain it would run in
/// `root`, from its rust-toolchain.toml or the default, without letting it
/// install that toolchain.
fn rustup_which(rustup: &Path, root: &Path) -> RustupWhich {
    let Ok(out) = std::process::Command::new(rustup)
        .args(["which", "rust-analyzer"])
        .current_dir(root)
        // As in `Client::start`: a toolchain pin inherited from `cargo run`
        // would outrank the checkout's own.
        .env_remove("RUSTUP_TOOLCHAIN")
        .env("RUSTUP_AUTO_INSTALL", "0")
        .output()
    else {
        return RustupWhich::Other;
    };
    if out.status.success() {
        let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        return if path.is_file() {
            RustupWhich::Found(path)
        } else {
            RustupWhich::Other
        };
    }
    rustup_which_failure(&String::from_utf8_lossy(&out.stderr))
}

/// Tell rustup's two missing cases apart by their `error:` lines, taking
/// the toolchain's name from its quotes and nothing else from the prose.
/// rustup 1.29 writes `unknown binary 'rust-analyzer' in toolchain '<t>'`
/// and `toolchain '<t>' is not installed`; older ones wrote
/// `'rust-analyzer' is not installed for the toolchain '<t>'`.
fn rustup_which_failure(stderr: &str) -> RustupWhich {
    for line in stderr.lines() {
        let Some(error) = line.trim().strip_prefix("error:") else {
            continue;
        };
        let Some(toolchain) = quoted_after(error, "toolchain '") else {
            continue;
        };
        let lower = error.to_ascii_lowercase();
        if lower.contains("rust-analyzer") {
            return RustupWhich::NoComponent(toolchain);
        }
        if lower.contains("is not installed") {
            return RustupWhich::NoToolchain(toolchain);
        }
    }
    RustupWhich::Other
}

/// The text between `opening` and the next `'`.
fn quoted_after(text: &str, opening: &str) -> Option<String> {
    let rest = &text[text.find(opening)? + opening.len()..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string()).filter(|t| !t.is_empty())
}

/// Whether `candidate` is rustup's proxy: the same file as `rustup`, as
/// rustup's links in `~/.cargo/bin` are, or a link to a file named so.
fn is_rustup_proxy(candidate: &Path, rustup: Option<&Path>) -> bool {
    let Ok(resolved) = std::fs::canonicalize(candidate) else {
        return false;
    };
    if resolved
        .file_name()
        .is_some_and(|name| name == "rustup" || name == "rustup-init")
    {
        return true;
    }
    let Some(rustup) = rustup.and_then(|r| std::fs::canonicalize(r).ok()) else {
        return false;
    };
    if resolved == rustup {
        return true;
    }
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(&resolved), std::fs::metadata(&rustup)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp directory with a `bin` on `PATH`, `home` with nothing in
    /// it, and Perch's servers under `servers`.
    struct Fake {
        dir: tempfile::TempDir,
    }

    impl Fake {
        fn new() -> Self {
            let fake = Fake {
                dir: tempfile::tempdir().unwrap(),
            };
            for sub in ["bin", "home", "servers", "checkout"] {
                std::fs::create_dir_all(fake.dir.path().join(sub)).unwrap();
            }
            fake
        }

        fn path(&self, sub: &str) -> PathBuf {
            self.dir.path().join(sub)
        }

        fn env(&self) -> Env {
            Env {
                path: Some(self.path("bin").into_os_string()),
                home: Some(self.path("home")),
                servers: self.path("servers"),
            }
        }

        /// An executable at `bin/name` that runs `script`.
        fn tool(&self, name: &str, script: &str) -> PathBuf {
            let path = self.path("bin").join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn rust(&self) -> Result<ServerSpec, Missing> {
            find(ServerKind::RustAnalyzer, &self.path("checkout"), &self.env())
        }
    }

    #[test]
    fn rust_analyzer_is_the_checkout_toolchains() {
        let fake = Fake::new();
        let toolchain = fake.path("home/toolchain-ra");
        std::fs::write(&toolchain, "").unwrap();
        fake.tool("rustup", &format!("echo {}", toolchain.display()));
        assert_eq!(fake.rust().unwrap().binary, toolchain);
    }

    #[test]
    fn a_toolchain_without_rust_analyzer_offers_the_component() {
        let fake = Fake::new();
        let rustup = fake.tool(
            "rustup",
            "echo \"error: unknown binary 'rust-analyzer' in toolchain '1.98.1-aarch64-apple-darwin'\" >&2; exit 1",
        );
        // A rust-analyzer from another release on PATH isn't used.
        fake.tool("rust-analyzer", "exit 0");
        let missing = fake.rust().unwrap_err();
        assert_eq!(missing.describe(), "rust-analyzer isn't installed for Rust 1.98.1.");
        assert_eq!(
            missing.install.as_deref(),
            Some(&Install::Command {
                program: rustup,
                args: ["component", "add", "rust-analyzer", "--toolchain", "1.98.1-aarch64-apple-darwin"]
                    .map(String::from)
                    .to_vec(),
                env: Vec::new(),
                into: None,
            })
        );
        assert_eq!(
            missing.install.unwrap().shown(),
            "rustup component add rust-analyzer --toolchain 1.98.1-aarch64-apple-darwin"
        );
    }

    #[test]
    fn a_missing_toolchain_offers_the_toolchain_with_rust_analyzer() {
        let fake = Fake::new();
        fake.tool(
            "rustup",
            "echo \"error: toolchain '1.80.0-aarch64-apple-darwin' is not installed\" >&2\n\
             echo 'help: run `rustup toolchain install` to install it' >&2; exit 1",
        );
        let missing = fake.rust().unwrap_err();
        assert_eq!(missing.toolchain.as_deref(), Some("1.80.0-aarch64-apple-darwin"));
        assert_eq!(
            missing.install.unwrap().shown(),
            "rustup toolchain install 1.80.0-aarch64-apple-darwin --profile minimal --component rust-analyzer"
        );
    }

    #[test]
    fn rustups_older_wording_is_read_too() {
        assert_eq!(
            rustup_which_failure(
                "error: 'rust-analyzer' is not installed for the toolchain 'stable-aarch64-apple-darwin'"
            ),
            RustupWhich::NoComponent("stable-aarch64-apple-darwin".into())
        );
        assert_eq!(rustup_which_failure("error: something else"), RustupWhich::Other);
        assert_eq!(rustup_which_failure("note: toolchain 'x' is not installed"), RustupWhich::Other);
    }

    /// Any other rustup failure is as good as no rustup: PATH, but never
    /// rustup's proxy on it.
    #[test]
    fn without_a_rustup_answer_path_is_used_but_not_rustups_proxy() {
        let fake = Fake::new();
        let rustup = fake.tool("rustup", "echo 'error: no default toolchain' >&2; exit 1");
        let proxies = fake.path("proxies");
        std::fs::create_dir_all(&proxies).unwrap();
        std::fs::hard_link(&rustup, proxies.join("rust-analyzer")).unwrap();
        let real = fake.path("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("rust-analyzer"), "").unwrap();
        let mut env = fake.env();
        env.path = Some(std::env::join_paths([fake.path("bin"), proxies, real.clone()]).unwrap());
        let spec = find(ServerKind::RustAnalyzer, &fake.path("checkout"), &env).unwrap();
        assert_eq!(spec.binary, real.join("rust-analyzer"));

        // A symlink to rustup is a proxy too, as `~/.cargo/bin`'s are.
        let cargo_bin = fake.path("home/.cargo/bin");
        std::fs::create_dir_all(&cargo_bin).unwrap();
        std::os::unix::fs::symlink(&rustup, cargo_bin.join("rust-analyzer")).unwrap();
        assert!(fake.rust().is_err());
    }

    #[test]
    fn without_rustup_rust_analyzer_needs_it() {
        let fake = Fake::new();
        let missing = fake.rust().unwrap_err();
        assert_eq!(missing.needs, Some(Need::Rustup));
        assert_eq!(missing.install, None);
        assert_eq!(missing.describe(), "rust-analyzer needs rustup, which isn't on your PATH.");
        fake.tool("rust-analyzer", "exit 0");
        assert!(fake.rust().is_ok());
    }

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
        assert!(!Status::Missing(Missing {
            kind: ServerKind::RustAnalyzer,
            needs: None,
            install: None,
            toolchain: None,
        })
        .is_usable());
        assert!(!Status::Unhealthy { message: "no workspace".into() }.is_usable());
    }
}
