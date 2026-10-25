//! Finding and describing language servers.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A language server Perch knows how to run. A worktree runs at most one
/// of each kind, rooted at the worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ServerKind {
    RustAnalyzer,
    Gopls,
    /// TypeScript and JavaScript both.
    TypescriptLanguageServer,
    Jdtls,
    /// Astral's type checker, which serves Python.
    Ty,
}

impl ServerKind {
    pub const ALL: [ServerKind; 5] = [
        ServerKind::RustAnalyzer,
        ServerKind::Gopls,
        ServerKind::TypescriptLanguageServer,
        ServerKind::Jdtls,
        ServerKind::Ty,
    ];

    /// The server's own name, as statuses and toasts say it.
    pub fn name(self) -> &'static str {
        match self {
            ServerKind::RustAnalyzer => "rust-analyzer",
            ServerKind::Gopls => "gopls",
            ServerKind::TypescriptLanguageServer => "typescript-language-server",
            ServerKind::Jdtls => "jdtls",
            ServerKind::Ty => "ty",
        }
    }

    /// What of a checkout's own code the server runs as it loads it, as a
    /// toast says it, or `None` for one that runs none. rust-analyzer
    /// builds and runs build scripts and proc macros, and follows the
    /// checkout's `.cargo/config.toml` and toolchain file; jdtls imports a
    /// Gradle or Maven build. On code the reader hasn't trusted these wait.
    pub fn project_code(self) -> Option<&'static str> {
        match self {
            ServerKind::RustAnalyzer => Some("build scripts and proc macros"),
            ServerKind::Jdtls => Some("Gradle or Maven build"),
            ServerKind::Gopls | ServerKind::TypescriptLanguageServer | ServerKind::Ty => None,
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
            "go" | "golang" => Some(ServerKind::Gopls),
            "typescript" | "ts" | "tsx" | "javascript" | "js" | "jsx" | "mjs" | "cjs" | "mts"
            | "cts" => Some(ServerKind::TypescriptLanguageServer),
            "java" => Some(ServerKind::Jdtls),
            "python" | "py" => Some(ServerKind::Ty),
            _ => None,
        }
    }

    /// The files at a worktree's root that say a project of this kind is
    /// there.
    pub fn markers(self) -> &'static [&'static str] {
        match self {
            ServerKind::RustAnalyzer => &["Cargo.toml"],
            ServerKind::Gopls => &["go.mod", "go.work"],
            ServerKind::TypescriptLanguageServer => {
                &["tsconfig.json", "jsconfig.json", "package.json"]
            }
            ServerKind::Jdtls => &[
                "pom.xml",
                "build.gradle",
                "build.gradle.kts",
                "settings.gradle",
                "settings.gradle.kts",
            ],
            ServerKind::Ty => &[
                "pyproject.toml",
                "setup.py",
                "setup.cfg",
                "requirements.txt",
            ],
        }
    }

    /// How long the handshake waits for the server's answer.
    pub fn initialize_timeout(self) -> Duration {
        match self {
            ServerKind::RustAnalyzer | ServerKind::Gopls | ServerKind::Ty => Duration::from_secs(5),
            // tsserver starts under it, which takes a while on a large
            // project.
            ServerKind::TypescriptLanguageServer => Duration::from_secs(15),
            // A JVM, and Eclipse's bundles, start first.
            ServerKind::Jdtls => Duration::from_secs(60),
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
    // A class file jdtls gave the source of.
    if is_class_file(path) {
        return Some((ServerKind::Jdtls, "java"));
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let (_, ext) = name.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "rs" => Some((ServerKind::RustAnalyzer, "rust")),
        "go" => Some((ServerKind::Gopls, "go")),
        "ts" | "mts" | "cts" => Some((ServerKind::TypescriptLanguageServer, "typescript")),
        "tsx" => Some((ServerKind::TypescriptLanguageServer, "typescriptreact")),
        "js" | "mjs" | "cjs" => Some((ServerKind::TypescriptLanguageServer, "javascript")),
        "jsx" => Some((ServerKind::TypescriptLanguageServer, "javascriptreact")),
        "java" => Some((ServerKind::Jdtls, "java")),
        "py" | "pyi" | "pyw" => Some((ServerKind::Ty, "python")),
        _ => None,
    }
}

/// Whether `path` is a class file inside a jar or the JDK, as jdtls names
/// one: a `jdt://` URI, which Perch keeps without its query.
pub fn is_class_file(path: &str) -> bool {
    path.starts_with("jdt://")
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
    Indexing {
        percent: Option<u8>,
    },
    Ready,
    /// Running, but it says it cannot do its work: rust-analyzer whose
    /// workspace failed to load answers every ask with nothing.
    Unhealthy {
        message: String,
    },
    Failed {
        reason: String,
    },
    /// Not started: it would run the checkout's own code (see
    /// `ServerKind::project_code`), which the reader hasn't trusted.
    Held,
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
            Status::Held => format!("{name} waits for you to trust this code"),
        }
    }

    pub fn is_usable(&self) -> bool {
        matches!(self, Status::Indexing { .. } | Status::Ready)
    }

    /// Whether the server has yet to read the workspace: what it answers
    /// with nothing now may be there once it has.
    pub fn is_loading(&self) -> bool {
        matches!(self, Status::Starting | Status::Indexing { .. })
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
    /// macOS's tool that names a JDK by version.
    pub java_home_tool: PathBuf,
    /// `JAVA_HOME`.
    pub java_home: Option<PathBuf>,
    /// Directories of JDKs, one in each entry: where Linux distributions
    /// install them.
    pub jvm_dirs: Vec<PathBuf>,
}

impl Env {
    /// The process's `PATH` and home, with Perch's servers under
    /// `servers`.
    pub fn current(servers: PathBuf) -> Self {
        Env {
            path: std::env::var_os("PATH"),
            home: std::env::var_os("HOME").map(PathBuf::from),
            servers,
            java_home_tool: PathBuf::from("/usr/libexec/java_home"),
            java_home: std::env::var_os("JAVA_HOME").map(PathBuf::from),
            jvm_dirs: vec![PathBuf::from("/usr/lib/jvm")],
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
    Go,
    Node,
    Jdk,
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
            (Some(Need::Go), _) => format!("{name} needs Go, which isn't on your PATH."),
            (Some(Need::Node), _) => {
                format!("{name} needs Node and npm, which aren't on your PATH.")
            }
            (Some(Need::Jdk), _) => {
                format!("{name} needs a JDK 21 or later, which isn't installed.")
            }
            (None, Some(toolchain)) => {
                format!(
                    "{name} isn't installed for Rust {}.",
                    short_toolchain(toolchain)
                )
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
    /// Download the tarball at `url`, check it against `sha256`, and
    /// unpack it into `into`, by way of its `.partial` sibling.
    Download {
        url: String,
        sha256: String,
        into: PathBuf,
    },
}

impl Install {
    /// The version an install into Perch's directory puts there, named by
    /// the directory it goes in.
    pub fn version(&self) -> Option<&str> {
        let into = match self {
            Install::Command { into, .. } => into.as_ref()?,
            Install::Download { into, .. } => into,
        };
        into.file_name()?.to_str()
    }

    /// The command as the toast's tooltip shows it.
    pub fn shown(&self) -> String {
        match self {
            Install::Command {
                program, args, env, ..
            } => {
                let program = program.file_name().map_or_else(
                    || program.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                env.iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .chain(std::iter::once(program))
                    .chain(args.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            Install::Download { url, sha256, .. } => {
                format!("Download {url}, check its SHA-256 is {sha256}, and unpack it")
            }
        }
    }
}

/// The `.partial` sibling an install into `into` writes to first.
pub fn partial(into: &Path) -> PathBuf {
    let name = into
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
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
        ServerKind::Gopls => find_gopls(env),
        ServerKind::TypescriptLanguageServer => find_typescript_language_server(env),
        ServerKind::Jdtls => find_jdtls(root, env),
        ServerKind::Ty => find_ty(env),
    }
}

/// The jdtls Perch downloads, and the SHA-256 Eclipse publishes for it.
const JDTLS_PIN: &str = "1.61.0";
const JDTLS_TARBALL: &str = "jdt-language-server-1.61.0-202609031315.tar.gz";
const JDTLS_SHA256: &str = "338e7e73d61836651ba2453919a0d34fa763eb4e7c03342092309bffb8934c64";

/// jdtls: Homebrew's wrapper on `PATH`, else Perch's own, run directly on
/// a JDK 21 or later. Missing, Perch downloads Eclipse's release tarball.
/// Either way it keeps its workspace data for the checkout under
/// `~/.perch/servers/jdtls-data`, never in the worktree, where it would
/// be untracked files.
fn find_jdtls(root: &Path, env: &Env) -> Result<ServerSpec, Missing> {
    let kind = ServerKind::Jdtls;
    let options = serde_json::json!({
        // Definitions in a jar come back as `jdt://` URIs, whose source
        // `java/classFileContents` gives, rather than nothing.
        "extendedClientCapabilities": { "classFileContentsSupport": true },
        // Nothing is built: a build writes its classes into the checkout,
        // `target/classes` for Maven, and navigating needs none of them.
        "settings": {
            "java": {
                "autobuild": { "enabled": false },
                "import": { "generatesMetadataFilesAtProjectRoot": false }
            }
        },
    });
    let data = || jdtls_data(env, root).display().to_string();
    if let Some(wrapper) = env.which("jdtls") {
        return Ok(ServerSpec {
            kind,
            binary: wrapper,
            args: vec![
                format!("--jvm-arg={NO_METADATA_AT_ROOT}"),
                "-data".into(),
                data(),
            ],
            options,
        });
    }
    let Some(jdk) = find_jdk(env) else {
        return Err(Missing {
            kind,
            needs: Some(Need::Jdk),
            install: None,
            toolchain: None,
        });
    };
    let install = newest_install(env, kind).and_then(|(_, dir)| {
        let launcher = std::fs::read_dir(dir.join("plugins"))
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n.starts_with("org.eclipse.equinox.launcher_") && n.ends_with(".jar")
                })
            })?;
        Some((dir, launcher))
    });
    let Some((dir, launcher)) = install else {
        return Err(Missing {
            kind,
            needs: None,
            install: Some(Box::new(jdtls_install(env))),
            toolchain: None,
        });
    };
    let config = jdtls_config(std::env::consts::OS, std::env::consts::ARCH);
    let args = [
        NO_METADATA_AT_ROOT,
        "-Declipse.application=org.eclipse.jdt.ls.core.id1",
        "-Dosgi.bundles.defaultStartLevel=4",
        "-Declipse.product=org.eclipse.jdt.ls.core.product",
        "-Xmx2G",
        "--add-modules=ALL-SYSTEM",
        "--add-opens",
        "java.base/java.util=ALL-UNNAMED",
        "--add-opens",
        "java.base/java.lang=ALL-UNNAMED",
        "-jar",
    ]
    .into_iter()
    .map(String::from)
    .chain([
        launcher.display().to_string(),
        "-configuration".into(),
        dir.join(config).display().to_string(),
        "-data".into(),
        data(),
    ])
    .collect();
    Ok(ServerSpec {
        kind,
        binary: jdk.join("bin/java"),
        args,
        options,
    })
}

/// The launcher configuration Eclipse ships for a platform, by Rust's
/// names for its OS and architecture. The `config_ss_*` ones beside them
/// are for an install the user can't write to, which Perch's never is.
fn jdtls_config(os: &str, arch: &str) -> &'static str {
    let arm = arch == "aarch64";
    match os {
        "macos" if arm => "config_mac_arm",
        "macos" => "config_mac",
        "windows" => "config_win",
        _ if arm => "config_linux_arm",
        _ => "config_linux",
    }
}

/// Eclipse's `.project`, `.classpath` and `.settings` go in jdtls's data
/// directory, not the checkout, where they'd be untracked files. jdtls's
/// file system reads this as it starts, so it's a JVM property; the
/// setting of the same name comes too late.
const NO_METADATA_AT_ROOT: &str = "-Djava.import.generatesMetadataFilesAtProjectRoot=false";

/// Where jdtls keeps its workspace for the checkout at `root`: named by the
/// first 12 hex digits of the SHA-256 of its path, with the path written
/// beside it so a sweep can tell when the checkout is gone.
pub fn jdtls_data(env: &Env, root: &Path) -> PathBuf {
    let dir = jdtls_data_path(&env.servers, root);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join("worktree"), root.as_os_str().as_encoded_bytes());
    }
    dir
}

/// The download of the pinned jdtls into Perch's directory.
fn jdtls_install(env: &Env) -> Install {
    Install::Download {
        url: format!("https://download.eclipse.org/jdtls/milestones/{JDTLS_PIN}/{JDTLS_TARBALL}"),
        sha256: JDTLS_SHA256.into(),
        into: env.servers.join(ServerKind::Jdtls.name()).join(JDTLS_PIN),
    }
}

/// Where jdtls's workspace for the checkout at `root` is, made or not.
pub fn jdtls_data_path(servers: &Path, root: &Path) -> PathBuf {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(root.as_os_str().as_encoded_bytes());
    let name: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    servers.join("jdtls-data").join(name)
}

/// A JDK new enough to run jdtls. On macOS, the one its `java_home`
/// picks for 21 or later, else `JAVA_HOME`. Elsewhere `JAVA_HOME`, else
/// the `java` on `PATH`, which is the JDK the system was set to, else the
/// newest of the installed ones. macOS's `/usr/bin/java` asks to install
/// Java when there is none, so it is never run there.
fn find_jdk(env: &Env) -> Option<PathBuf> {
    let new_enough = |home: &PathBuf| jdk_major(home).is_some_and(|major| major >= 21);
    if cfg!(target_os = "macos") {
        let picked = std::process::Command::new(&env.java_home_tool)
            .args(["-v", "21+"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
            .filter(|home| home.join("bin/java").is_file());
        return picked.or_else(|| env.java_home.clone().filter(new_enough));
    }
    if let Some(home) = env.java_home.clone().filter(new_enough) {
        return Some(home);
    }
    // `/usr/bin/java` is a link, often by way of `/etc/alternatives`, into
    // the JDK's own `bin`.
    let on_path = env
        .which("java")
        .and_then(|java| std::fs::canonicalize(java).ok())
        .and_then(|java| Some(java.parent()?.parent()?.to_path_buf()))
        .filter(new_enough);
    on_path.or_else(|| {
        let mut homes: Vec<PathBuf> = env
            .jvm_dirs
            .iter()
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flatten()
            .flatten()
            .filter_map(|entry| std::fs::canonicalize(entry.path()).ok())
            .collect();
        homes.sort();
        homes.dedup();
        homes
            .into_iter()
            .filter_map(|home| Some((jdk_major(&home)?, home)))
            .filter(|(major, _)| *major >= 21)
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, home)| home)
    })
}

/// The major version of the JDK at `home`: from the `release` file every
/// JDK has, else from asking its `java`. `None` when there's no `java`.
fn jdk_major(home: &Path) -> Option<u64> {
    let java = home.join("bin/java");
    if !java.is_file() {
        return None;
    }
    let release = std::fs::read_to_string(home.join("release")).unwrap_or_default();
    if let Some(line) = release.lines().find(|l| l.starts_with("JAVA_VERSION=")) {
        return java_major(line);
    }
    let out = std::process::Command::new(java)
        .arg("-version")
        .output()
        .ok()?;
    // `java -version` writes to stderr.
    java_major(&String::from_utf8_lossy(&out.stderr))
}

/// The major version in `java -version`'s `version "21.0.2"`, or 8 for
/// the old `version "1.8.0_402"`.
fn java_major(text: &str) -> Option<u64> {
    let quoted = text.split('"').nth(1)?;
    let mut parts = quoted.split(['.', '-', '+']);
    match parts.next()?.parse().ok()? {
        1 => parts.next()?.parse().ok(),
        major => Some(major),
    }
}

/// A server Perch installed that is older than the version Perch now
/// pins, and the install that brings it up to date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outdated {
    pub kind: ServerKind,
    pub installed: String,
    pub pin: String,
    pub install: Install,
}

impl Outdated {
    /// The line its toast shows.
    pub fn describe(&self) -> String {
        format!("{} {} is available.", self.kind.name(), self.pin)
    }
}

/// Whether the server `spec` runs is one of Perch's own installs that is
/// older than the pin. A server Perch didn't install is never outdated:
/// it isn't Perch's to update.
pub fn outdated(spec: &ServerSpec, env: &Env) -> Option<Outdated> {
    let kind = spec.kind;
    let (installed, dir) = newest_install(env, kind)?;
    // The binary, or for jdtls the launcher jar it runs, is in the install.
    let runs_it = spec.binary.starts_with(&dir)
        || spec.args.iter().any(|arg| Path::new(arg).starts_with(&dir));
    if !runs_it {
        return None;
    }
    let install = match kind {
        ServerKind::RustAnalyzer => return None,
        ServerKind::Gopls => gopls_install(&env.which("go")?, env),
        ServerKind::TypescriptLanguageServer => typescript_install(&env.which("npm")?, env),
        ServerKind::Jdtls => jdtls_install(env),
        ServerKind::Ty => ty_install(env),
    };
    let pin = install.version()?.to_string();
    (compare_versions(&installed, &pin) == std::cmp::Ordering::Less).then_some(Outdated {
        kind,
        installed,
        pin,
        install,
    })
}

/// Tidy Perch's servers directory as Perch starts, when nothing runs
/// from it: installs cut short (`.partial`), every version of a server but
/// its newest, and jdtls data whose checkout is gone.
pub fn sweep(servers: &Path) {
    let Ok(entries) = std::fs::read_dir(servers) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if dir.file_name().is_some_and(|n| n == "jdtls-data") {
            sweep_jdtls_data(&dir);
            continue;
        }
        for partial in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = partial.file_name();
            if name.to_string_lossy().ends_with(".partial") {
                remove(&partial.path());
            }
        }
        let mut versions = installs(&dir);
        versions.pop();
        for (_, old) in versions {
            remove(&old);
        }
    }
}

/// Remove each jdtls workspace whose checkout is no longer there.
fn sweep_jdtls_data(data: &Path) {
    for entry in std::fs::read_dir(data).into_iter().flatten().flatten() {
        let worktree = std::fs::read(entry.path().join("worktree"))
            .ok()
            .map(|bytes| PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()));
        if !worktree.is_some_and(|w| w.is_dir()) {
            remove(&entry.path());
        }
    }
}

fn remove(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => tracing::info!(dir = %dir.display(), "removed an old language server install"),
        Err(error) => tracing::warn!(%error, dir = %dir.display(), "could not remove"),
    }
}

/// What Perch says as a server starts, once: a TypeScript checkout without
/// its packages, whose imports from them can't resolve. Perch doesn't
/// install them, since that runs the pull request's install scripts.
pub fn start_warnings(kind: ServerKind, root: &Path) -> Vec<String> {
    match kind {
        ServerKind::TypescriptLanguageServer
            if root.join("package.json").is_file() && !root.join("node_modules").is_dir() =>
        {
            vec!["No node_modules in this checkout, so imports from packages won't resolve.".into()]
        }
        _ => Vec::new(),
    }
}

/// The typescript-language-server Perch installs, and the TypeScript
/// beside it. TypeScript 7 has no tsserver for it to run, so 6 it is.
const TYPESCRIPT_LANGUAGE_SERVER_PIN: &str = "6.0.1";
const TYPESCRIPT_PIN: &str = "6.0.3";

/// The first typescript-language-server to read `tsserver.path` from its
/// initialization options. An older one ignores it and runs the
/// checkout's TypeScript.
const TYPESCRIPT_LANGUAGE_SERVER_TAKES_TSSERVER: &str = "2.1.0";

/// typescript-language-server on `PATH` with the TypeScript it would load
/// by itself, else Perch's own with the TypeScript installed beside it.
/// Perch never runs a checkout's own TypeScript, since a pull request
/// could put any tsserver.js there; and it always names the tsserver,
/// since typescript-language-server would otherwise pick the checkout's
/// itself. So a server on `PATH` that can't be given one gives way to
/// Perch's: one with no TypeScript of its own, one too old to take the
/// name, or one Perch can't place in its package, such as a version
/// manager's shim. Missing, Perch installs both with npm into its own
/// directory, without their install scripts.
fn find_typescript_language_server(env: &Env) -> Result<ServerSpec, Missing> {
    let kind = ServerKind::TypescriptLanguageServer;
    let users = || {
        let binary = env.which(kind.name())?;
        let tsserver = typescript_of_server(&binary)?;
        Some((binary, tsserver))
    };
    let perchs = || {
        let (_, dir) = newest_install(env, kind)?;
        let binary = dir.join("node_modules/.bin").join(kind.name());
        let tsserver = tsserver(&dir.join("node_modules/typescript/lib"))?;
        binary.is_file().then_some((binary, tsserver))
    };
    if let Some((binary, tsserver)) = users().or_else(perchs) {
        return Ok(ServerSpec {
            kind,
            binary,
            args: vec!["--stdio".into()],
            options: serde_json::json!({
                "hostInfo": "perch",
                "tsserver": { "path": tsserver.display().to_string() },
            }),
        });
    }
    let (Some(npm), Some(_)) = (env.which("npm"), env.which("node")) else {
        return Err(Missing {
            kind,
            needs: Some(Need::Node),
            install: None,
            toolchain: None,
        });
    };
    Err(Missing {
        kind,
        needs: None,
        install: Some(Box::new(typescript_install(&npm, env))),
        toolchain: None,
    })
}

/// The tsserver of the TypeScript the typescript-language-server at
/// `binary` loads by itself, when that server takes the tsserver Perch
/// names. Node resolves it from the server's own file, its links
/// followed, through the `node_modules` of each directory above, and
/// stops at the first TypeScript there: the package's own, or a global
/// install's beside it. A server whose file isn't in its package, as a
/// wrapper script, might be any version, so it gets none.
fn typescript_of_server(binary: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(binary).ok()?;
    let package = real.ancestors().skip(1).find_map(|dir| {
        let package = package_json(&dir.join("package.json"))?;
        (package["name"] == "typescript-language-server").then_some(package)
    })?;
    let version = package["version"].as_str()?;
    if compare_versions(version, TYPESCRIPT_LANGUAGE_SERVER_TAKES_TSSERVER).is_lt() {
        return None;
    }
    let typescript = real
        .ancestors()
        .skip(1)
        .filter(|dir| dir.file_name().is_none_or(|name| name != "node_modules"))
        .map(|dir| dir.join("node_modules/typescript"))
        .find(|typescript| typescript.exists())?;
    tsserver(&typescript.join("lib"))
}

/// `lib`'s tsserver.js, its links resolved, when typescript-language-server
/// would take it as the tsserver Perch names: the package it's in says its
/// version. The server reads that same package.json, and given a tsserver
/// whose package has no version it warns and falls back to the checkout's.
fn tsserver(lib: &Path) -> Option<PathBuf> {
    let tsserver = std::fs::canonicalize(lib.join("tsserver.js")).ok()?;
    let package = package_json(&tsserver.parent()?.parent()?.join("package.json"))?;
    let versioned = package["version"].as_str().is_some_and(|v| !v.is_empty());
    let named = tsserver
        .file_name()
        .is_some_and(|name| name == "tsserver.js");
    (versioned && named && tsserver.is_file()).then_some(tsserver)
}

/// The package.json at `path`, parsed.
fn package_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// npm's install of typescript-language-server and its TypeScript into
/// Perch's directory, without their install scripts.
fn typescript_install(npm: &Path, env: &Env) -> Install {
    let kind = ServerKind::TypescriptLanguageServer;
    let into = env
        .servers
        .join(kind.name())
        .join(TYPESCRIPT_LANGUAGE_SERVER_PIN);
    Install::Command {
        program: npm.to_path_buf(),
        args: vec![
            "install".into(),
            "--prefix".into(),
            partial(&into).display().to_string(),
            "--ignore-scripts".into(),
            "--no-fund".into(),
            "--no-audit".into(),
            format!("typescript-language-server@{TYPESCRIPT_LANGUAGE_SERVER_PIN}"),
            format!("typescript@{TYPESCRIPT_PIN}"),
        ],
        env: Vec::new(),
        into: Some(into),
    }
}

/// The gopls Perch installs, newest first, each with the oldest Go that
/// builds it. Bumping one is a commit, like bumping a dependency.
const GOPLS_PINS: &[((u64, u64), &str)] = &[((1, 26), "0.23.0"), ((1, 25), "0.21.1")];

/// The gopls the Go at `go` builds: the newest pin it is new enough for,
/// else the oldest, whose install then says which Go it wants.
fn gopls_pin(go: &Path) -> &'static str {
    let version = go_version(go);
    GOPLS_PINS
        .iter()
        .find(|(needs, _)| version.is_some_and(|v| v >= *needs))
        .or(GOPLS_PINS.last())
        .map_or("0.23.0", |(_, pin)| pin)
}

/// The local Go's major and minor version, from `go version`. Asked with
/// `GOTOOLCHAIN=local` and outside any module, so the go command doesn't
/// switch to, and download, a toolchain some go.mod asks for.
fn go_version(go: &Path) -> Option<(u64, u64)> {
    let out = std::process::Command::new(go)
        .arg("version")
        .env("GOTOOLCHAIN", "local")
        .current_dir(std::env::temp_dir())
        .output()
        .ok()?;
    parse_go_version(&String::from_utf8_lossy(&out.stdout))
}

/// `go version go1.25.0 darwin/arm64` as `(1, 25)`.
fn parse_go_version(text: &str) -> Option<(u64, u64)> {
    let word = text
        .split_whitespace()
        .find_map(|w| w.strip_prefix("go1."))?;
    let minor = word
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((1, minor))
}

/// gopls on `PATH`, else in Go's default install directory, `~/go/bin`,
/// which a Finder launch has no `PATH` to, else Perch's own. Missing, Perch
/// installs it with `go install` into its own directory.
fn find_gopls(env: &Env) -> Result<ServerSpec, Missing> {
    let kind = ServerKind::Gopls;
    let users = env.which("gopls").or_else(|| {
        env.home
            .as_ref()
            .map(|home| home.join("go/bin/gopls"))
            .filter(|p| p.is_file())
    });
    let perchs = || {
        newest_install(env, kind)
            .map(|(_, dir)| dir.join("gopls"))
            .filter(|p| p.is_file())
    };
    if let Some(binary) = users.or_else(perchs) {
        return Ok(ServerSpec {
            kind,
            binary,
            args: Vec::new(),
            options: serde_json::Value::Null,
        });
    }
    let Some(go) = env.which("go") else {
        return Err(Missing {
            kind,
            needs: Some(Need::Go),
            install: None,
            toolchain: None,
        });
    };
    Err(Missing {
        kind,
        needs: None,
        install: Some(Box::new(gopls_install(&go, env))),
        toolchain: None,
    })
}

/// `go install` of the gopls `go` builds, into Perch's directory. Never
/// with a Go toolchain the go command would download for it.
pub(crate) fn gopls_install(go: &Path, env: &Env) -> Install {
    let pin = gopls_pin(go);
    let into = env.servers.join(ServerKind::Gopls.name()).join(pin);
    Install::Command {
        program: go.to_path_buf(),
        args: vec!["install".into(), format!("golang.org/x/tools/gopls@v{pin}")],
        env: vec![
            ("GOBIN".into(), partial(&into).display().to_string()),
            ("GOTOOLCHAIN".into(), "local".into()),
        ],
        into: Some(into),
    }
}

/// The ty Perch downloads: the pinned release, and for each processor its
/// tarball's name and the SHA-256 Astral publishes for it. 0.0.86 is the
/// first release whose macOS binaries are signed and notarized.
const TY_PIN: &str = "0.0.86";
const TY_AARCH64: (&str, &str) = (
    "aarch64",
    "ba782554d855aa69ce6ec8fa5898f5256a8603eb81f35d1927434576a434ae4e",
);
const TY_X86_64: (&str, &str) = (
    "x86_64",
    "6b7908dec8629f9c18fe8c76fdcc22497ff823b14c351636aebe4d552d0e1fe9",
);

/// The release tarball for the processor Perch runs on: arm64 or x86_64.
fn ty_release() -> (&'static str, &'static str) {
    if std::env::consts::ARCH == "aarch64" {
        TY_AARCH64
    } else {
        TY_X86_64
    }
}

/// ty on `PATH`, else Perch's own install, else a download of the pinned
/// release. ty runs none of the checkout's code, so it starts without a
/// trust prompt.
fn find_ty(env: &Env) -> Result<ServerSpec, Missing> {
    let kind = ServerKind::Ty;
    let perchs = || {
        newest_install(env, kind)
            .map(|(_, dir)| ty_binary(&dir))
            .filter(|p| p.is_file())
    };
    if let Some(binary) = env.which("ty").or_else(perchs) {
        return Ok(ServerSpec {
            kind,
            binary,
            args: vec!["server".into()],
            options: serde_json::Value::Null,
        });
    }
    Err(Missing {
        kind,
        needs: None,
        install: Some(Box::new(ty_install(env))),
        toolchain: None,
    })
}

/// The ty binary in an install's folder. The release tarball unpacks to a
/// folder named for its target, as `ty-aarch64-apple-darwin/ty`.
fn ty_binary(dir: &Path) -> PathBuf {
    let (arch, _) = ty_release();
    dir.join(format!("ty-{arch}-apple-darwin")).join("ty")
}

/// The download of the pinned ty for this processor, into Perch's
/// directory.
fn ty_install(env: &Env) -> Install {
    let (arch, sha256) = ty_release();
    Install::Download {
        url: format!(
            "https://github.com/astral-sh/ty/releases/download/{TY_PIN}/ty-{arch}-apple-darwin.tar.gz"
        ),
        sha256: sha256.into(),
        into: env.servers.join(ServerKind::Ty.name()).join(TY_PIN),
    }
}

/// The newest version of `kind` under Perch's servers directory, and its
/// directory. An install still under way, `.partial`, isn't one.
pub fn newest_install(env: &Env, kind: ServerKind) -> Option<(String, PathBuf)> {
    installs(&env.servers.join(kind.name()))
        .into_iter()
        .next_back()
}

/// The finished installs in a server's directory, oldest first.
pub fn installs(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            (!name.ends_with(".partial")).then(|| (name, entry.path()))
        })
        .collect();
    found.sort_by(|a, b| compare_versions(&a.0, &b.0));
    found
}

/// Dotted versions compared number by number, `0.23.0` after `0.9.1`.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let numbers = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split(['.', '-'])
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    numbers(a).cmp(&numbers(b)).then_with(|| a.cmp(b))
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
        .filter(|kind| {
            kind.markers()
                .iter()
                .any(|marker| root.join(marker).is_file())
        })
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
            for sub in ["bin", "home", "servers", "checkout", "jvm"] {
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
                java_home_tool: self.path("bin/java_home"),
                java_home: None,
                jvm_dirs: vec![self.path("jvm")],
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

        /// A JDK of `version` at `sub`, saying so in its `release` file.
        fn jdk(&self, sub: &str, version: &str) -> PathBuf {
            let home = self.path(sub);
            std::fs::create_dir_all(home.join("bin")).unwrap();
            std::fs::write(home.join("bin/java"), "").unwrap();
            std::fs::write(
                home.join("release"),
                format!("JAVA_VERSION=\"{version}\"\n"),
            )
            .unwrap();
            home
        }

        fn rust(&self) -> Result<ServerSpec, Missing> {
            find(
                ServerKind::RustAnalyzer,
                &self.path("checkout"),
                &self.env(),
            )
        }

        /// A TypeScript package at `sub` with a tsserver, and
        /// `package_json`; its tsserver.js, links resolved.
        fn typescript(&self, sub: &str, package_json: &str) -> PathBuf {
            let lib = self.path(sub).join("lib");
            std::fs::create_dir_all(&lib).unwrap();
            std::fs::write(lib.join("tsserver.js"), "").unwrap();
            std::fs::write(self.path(sub).join("package.json"), package_json).unwrap();
            std::fs::canonicalize(lib.join("tsserver.js")).unwrap()
        }

        /// typescript-language-server `version` in a global npm install
        /// under `global`, linked from `bin` as npm links it: the link, and
        /// the package's directory.
        fn global_typescript_server(&self, version: &str) -> (PathBuf, PathBuf) {
            let package = self.path("global/lib/node_modules/typescript-language-server");
            std::fs::create_dir_all(package.join("lib")).unwrap();
            std::fs::write(package.join("lib/cli.mjs"), "").unwrap();
            let manifest =
                format!(r#"{{ "name": "typescript-language-server", "version": "{version}" }}"#);
            std::fs::write(package.join("package.json"), manifest).unwrap();
            let link = self.path("bin/typescript-language-server");
            if link.symlink_metadata().is_err() {
                std::os::unix::fs::symlink(package.join("lib/cli.mjs"), &link).unwrap();
            }
            (link, package)
        }

        /// Perch's own install of typescript-language-server `version`,
        /// with its TypeScript: the server's binary.
        fn perchs_typescript_server(&self, version: &str) -> PathBuf {
            let into = format!("servers/typescript-language-server/{version}");
            let bin = self.path(&into).join("node_modules/.bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("typescript-language-server"), "").unwrap();
            self.typescript(
                &format!("{into}/node_modules/typescript"),
                TYPESCRIPT_PACKAGE,
            );
            bin.join("typescript-language-server")
        }
    }

    const TYPESCRIPT_PACKAGE: &str = r#"{ "name": "typescript", "version": "6.0.3" }"#;

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
        assert_eq!(
            missing.describe(),
            "rust-analyzer isn't installed for Rust 1.98.1."
        );
        assert_eq!(
            missing.install.as_deref(),
            Some(&Install::Command {
                program: rustup,
                args: [
                    "component",
                    "add",
                    "rust-analyzer",
                    "--toolchain",
                    "1.98.1-aarch64-apple-darwin"
                ]
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
        assert_eq!(
            missing.toolchain.as_deref(),
            Some("1.80.0-aarch64-apple-darwin")
        );
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
        assert_eq!(
            rustup_which_failure("error: something else"),
            RustupWhich::Other
        );
        assert_eq!(
            rustup_which_failure("note: toolchain 'x' is not installed"),
            RustupWhich::Other
        );
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

    /// gopls: the user's on PATH, else in `~/go/bin`, else Perch's newest;
    /// missing, `go install` into Perch's directory, or nothing without Go.
    #[test]
    fn gopls_is_the_users_else_perchs_else_installed_with_go() {
        let fake = Fake::new();
        let gopls = |fake: &Fake| find(ServerKind::Gopls, &fake.path("checkout"), &fake.env());
        let missing = gopls(&fake).unwrap_err();
        assert_eq!(missing.needs, Some(Need::Go));
        assert_eq!(
            missing.describe(),
            "gopls needs Go, which isn't on your PATH."
        );

        fake.tool("go", "echo go version go1.25.0 darwin/arm64");
        let missing = gopls(&fake).unwrap_err();
        assert_eq!(missing.describe(), "gopls isn't installed.");
        assert_eq!(
            missing.install.as_deref().unwrap().shown(),
            format!(
                "GOBIN={} GOTOOLCHAIN=local go install golang.org/x/tools/gopls@v0.21.1",
                partial(&fake.path("servers/gopls/0.21.1")).display()
            )
        );
        fake.tool("go", "echo go version go1.27.2 darwin/arm64");
        let into = fake.path("servers/gopls/0.23.0");
        assert!(gopls(&fake)
            .unwrap_err()
            .install
            .unwrap()
            .shown()
            .ends_with("gopls@v0.23.0"));

        // Perch's newest, with an install under way and an older one beside.
        for version in ["0.9.1", "0.23.0", "0.24.0.partial"] {
            let dir = fake.path("servers/gopls").join(version);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("gopls"), "").unwrap();
        }
        assert_eq!(gopls(&fake).unwrap().binary, into.join("gopls"));

        let users = fake.path("home/go/bin");
        std::fs::create_dir_all(&users).unwrap();
        std::fs::write(users.join("gopls"), "").unwrap();
        assert_eq!(gopls(&fake).unwrap().binary, users.join("gopls"));
        let on_path = fake.tool("gopls", "exit 0");
        assert_eq!(gopls(&fake).unwrap().binary, on_path);
    }

    /// ty: the user's on PATH, else Perch's own install, else the pinned
    /// release downloaded with its checksum for this processor.
    #[test]
    fn ty_is_the_users_else_perchs_else_downloaded() {
        let fake = Fake::new();
        let (arch, sha256) = if std::env::consts::ARCH == "aarch64" {
            (
                "aarch64",
                "ba782554d855aa69ce6ec8fa5898f5256a8603eb81f35d1927434576a434ae4e",
            )
        } else {
            (
                "x86_64",
                "6b7908dec8629f9c18fe8c76fdcc22497ff823b14c351636aebe4d552d0e1fe9",
            )
        };
        let ty = |fake: &Fake| find(ServerKind::Ty, &fake.path("checkout"), &fake.env());
        let missing = ty(&fake).unwrap_err();
        assert_eq!(missing.describe(), "ty isn't installed.");
        assert_eq!(
            missing.install.as_deref(),
            Some(&Install::Download {
                url: format!(
                    "https://github.com/astral-sh/ty/releases/download/0.0.86/ty-{arch}-apple-darwin.tar.gz"
                ),
                sha256: sha256.into(),
                into: fake.path("servers/ty/0.0.86"),
            })
        );

        // Perch's own, where the tarball unpacks it.
        let binary = fake.path(&format!("servers/ty/0.0.86/ty-{arch}-apple-darwin/ty"));
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, "").unwrap();
        let spec = ty(&fake).unwrap();
        assert_eq!(spec.binary, binary);
        assert_eq!(spec.kind, ServerKind::Ty);
        assert_eq!(spec.args, ["server"]);
        assert_eq!(spec.options, serde_json::Value::Null);

        // The user's, on PATH.
        let users = fake.tool("ty", "exit 0");
        assert_eq!(ty(&fake).unwrap().binary, users);
    }

    /// Only Perch's own ty, older than the pin, is offered the pin.
    #[test]
    fn an_old_install_of_ty_is_offered_the_pin() {
        let fake = Fake::new();
        let (arch, _) = ty_release();
        let old = fake.path(&format!("servers/ty/0.0.85/ty-{arch}-apple-darwin/ty"));
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "").unwrap();
        let spec = find(ServerKind::Ty, &fake.path("checkout"), &fake.env()).unwrap();
        let old_one = outdated(&spec, &fake.env()).unwrap();
        assert_eq!(old_one.installed, "0.0.85");
        assert_eq!(old_one.pin, "0.0.86");
        assert_eq!(old_one.describe(), "ty 0.0.86 is available.");
        assert_eq!(old_one.install, ty_install(&fake.env()));

        // The user's own: never Perch's to update.
        fake.tool("ty", "exit 0");
        let spec = find(ServerKind::Ty, &fake.path("checkout"), &fake.env()).unwrap();
        assert_eq!(outdated(&spec, &fake.env()), None);
    }

    /// The TypeScript server: the user's with the TypeScript it would load
    /// itself, else Perch's with the one beside it, and never the
    /// checkout's; missing, npm into Perch's directory without install
    /// scripts, or nothing without Node.
    #[test]
    fn the_typescript_server_never_runs_the_checkouts_typescript() {
        let fake = Fake::new();
        let kind = ServerKind::TypescriptLanguageServer;
        let ts = |fake: &Fake| find(kind, &fake.path("checkout"), &fake.env());
        // One a pull request could have put there.
        fake.typescript("checkout/node_modules/typescript", TYPESCRIPT_PACKAGE);
        assert_eq!(ts(&fake).unwrap_err().needs, Some(Need::Node));
        let npm = fake.tool("npm", "exit 0");
        assert_eq!(
            ts(&fake).unwrap_err().needs,
            Some(Need::Node),
            "npm without node"
        );
        fake.tool("node", "exit 0");
        let missing = ts(&fake).unwrap_err();
        let into = fake.path("servers/typescript-language-server/6.0.1");
        assert_eq!(
            missing.install.unwrap().shown(),
            format!(
                "npm install --prefix {} --ignore-scripts --no-fund --no-audit \
                 typescript-language-server@6.0.1 typescript@6.0.3",
                partial(&into).display()
            )
        );

        // Perch's own, once the TypeScript installed beside it says its
        // version.
        let bin = into.join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("typescript-language-server"), "").unwrap();
        let perchs = "servers/typescript-language-server/6.0.1/node_modules/typescript";
        fake.typescript(perchs, "{}");
        assert!(ts(&fake).is_err(), "nor the checkout's instead");
        let beside = fake.typescript(perchs, TYPESCRIPT_PACKAGE);
        let spec = ts(&fake).unwrap();
        assert_eq!(spec.binary, bin.join("typescript-language-server"));
        assert_eq!(spec.args, ["--stdio"]);
        assert_eq!(
            spec.options["tsserver"]["path"],
            beside.display().to_string()
        );

        // The user's, linked from PATH. With no TypeScript of its own, as
        // a version manager's shim, Perch's runs instead. One beside the
        // link isn't the server's: node follows the link first.
        let (users, package) = fake.global_typescript_server("6.0.1");
        fake.typescript("bin/node_modules/typescript", TYPESCRIPT_PACKAGE);
        assert_eq!(
            ts(&fake).unwrap().binary,
            bin.join("typescript-language-server")
        );
        // A global install's TypeScript beside the server's package.
        let globals = "global/lib/node_modules/typescript";
        fake.typescript(globals, r#"{ "version": "" }"#);
        assert_eq!(
            ts(&fake).unwrap().binary,
            bin.join("typescript-language-server")
        );
        let global = fake.typescript(globals, TYPESCRIPT_PACKAGE);
        let spec = ts(&fake).unwrap();
        assert_eq!(spec.binary, users);
        assert_eq!(
            spec.options["tsserver"]["path"],
            global.display().to_string()
        );
        // The package's own, nearer, as node would find it first. Node
        // stops there, so one with no tsserver, as 7, isn't passed over
        // for the global one.
        let owns = "global/lib/node_modules/typescript-language-server/node_modules/typescript";
        let own = fake.typescript(owns, TYPESCRIPT_PACKAGE);
        assert_eq!(
            ts(&fake).unwrap().options["tsserver"]["path"],
            own.display().to_string()
        );
        std::fs::remove_file(&own).unwrap();
        assert_eq!(
            ts(&fake).unwrap().binary,
            bin.join("typescript-language-server")
        );

        // With no TypeScript of its own and no Perch's, it's missing.
        std::fs::remove_dir_all(fake.path(globals)).unwrap();
        std::fs::remove_dir_all(package.join("node_modules")).unwrap();
        std::fs::remove_dir_all(fake.path("servers/typescript-language-server")).unwrap();
        let missing = ts(&fake).unwrap_err();
        assert_eq!(missing.needs, None);
        assert_eq!(
            missing.install.as_deref(),
            Some(&typescript_install(&npm, &fake.env()))
        );
    }

    /// typescript-language-server takes a tsserver only when the package
    /// it's in says its version, and falls back to the checkout's
    /// otherwise. A linked one is named as the file it links to.
    #[test]
    fn a_typescript_says_its_version() {
        let fake = Fake::new();
        let file = fake.typescript("typescript", TYPESCRIPT_PACKAGE);
        let lib = file.parent().unwrap().to_path_buf();
        assert_eq!(tsserver(&lib), Some(file.clone()));
        for package in [
            "",
            "nonsense",
            "[]",
            "{}",
            r#"{ "version": "" }"#,
            r#"{ "version": 6 }"#,
        ] {
            fake.typescript("typescript", package);
            assert_eq!(tsserver(&lib), None, "{package}");
        }
        fake.typescript("typescript", TYPESCRIPT_PACKAGE);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(tsserver(&lib), None, "no tsserver.js");

        let elsewhere = fake.typescript("elsewhere", "{}");
        std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
        assert_eq!(tsserver(&lib), None, "linked to one without a version");
        fake.typescript("elsewhere", TYPESCRIPT_PACKAGE);
        assert_eq!(tsserver(&lib), Some(elsewhere));
    }

    /// A server on `PATH` older than 2.1, which ignores the tsserver Perch
    /// names, or one Perch can't place in its package, as a wrapper
    /// script, gives way to Perch's, whatever TypeScript is near it; with
    /// no Perch's, it's missing.
    #[test]
    fn an_old_or_wrapped_server_on_path_gives_way_to_perchs() {
        let fake = Fake::new();
        let kind = ServerKind::TypescriptLanguageServer;
        let ts = |fake: &Fake| find(kind, &fake.path("checkout"), &fake.env());
        let npm = fake.tool("npm", "exit 0");
        fake.tool("node", "exit 0");
        let (users, _) = fake.global_typescript_server("2.0.1");
        fake.typescript("global/lib/node_modules/typescript", TYPESCRIPT_PACKAGE);
        let missing = ts(&fake).unwrap_err();
        assert_eq!(
            missing.install.as_deref(),
            Some(&typescript_install(&npm, &fake.env()))
        );
        let perchs = fake.perchs_typescript_server("6.0.1");
        assert_eq!(ts(&fake).unwrap().binary, perchs);
        fake.global_typescript_server("2.1.0");
        assert_eq!(ts(&fake).unwrap().binary, users);

        std::fs::remove_file(&users).unwrap();
        fake.tool(
            "typescript-language-server",
            "exec node /elsewhere/lib/cli.mjs \"$@\"",
        );
        fake.typescript("node_modules/typescript", TYPESCRIPT_PACKAGE);
        assert_eq!(ts(&fake).unwrap().binary, perchs);
    }

    #[test]
    fn a_checkout_without_its_packages_is_warned_about() {
        let fake = Fake::new();
        let root = fake.path("checkout");
        let kind = ServerKind::TypescriptLanguageServer;
        assert!(start_warnings(kind, &root).is_empty());
        std::fs::write(root.join("package.json"), "{}").unwrap();
        assert_eq!(start_warnings(kind, &root).len(), 1);
        assert!(start_warnings(ServerKind::Gopls, &root).is_empty());
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        assert!(start_warnings(kind, &root).is_empty());
    }

    /// jdtls: Homebrew's wrapper, else Perch's own on a JDK 21 or later;
    /// missing, a checked download, or nothing without a JDK. Its data is
    /// outside the checkout, one directory per checkout.
    #[test]
    fn jdtls_runs_on_a_jdk_with_its_data_outside_the_checkout() {
        let fake = Fake::new();
        let root = fake.path("checkout");
        let jdtls = |fake: &Fake| find(ServerKind::Jdtls, &root, &fake.env());
        assert_eq!(jdtls(&fake).unwrap_err().needs, Some(Need::Jdk));

        // Where macOS's `java_home` and Linux's look both find it.
        let jdk = fake.jdk("jvm/jdk-21", "21.0.2");
        fake.tool("java_home", &format!("echo {}", jdk.display()));
        let missing = jdtls(&fake).unwrap_err();
        let into = fake.path("servers/jdtls/1.61.0");
        assert_eq!(
            missing.install.as_deref(),
            Some(&Install::Download {
                url: "https://download.eclipse.org/jdtls/milestones/1.61.0/\
                      jdt-language-server-1.61.0-202609031315.tar.gz"
                    .into(),
                sha256: JDTLS_SHA256.into(),
                into: into.clone(),
            })
        );

        let plugins = into.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        let launcher = plugins.join("org.eclipse.equinox.launcher_1.7.0.v20250424-1814.jar");
        std::fs::write(&launcher, "").unwrap();
        let spec = jdtls(&fake).unwrap();
        assert_eq!(spec.binary, jdk.join("bin/java"));
        assert_eq!(spec.args[0], NO_METADATA_AT_ROOT);
        let data = jdtls_data(&fake.env(), &root);
        assert!(data.starts_with(fake.path("servers/jdtls-data")));
        assert_eq!(
            std::fs::read_to_string(data.join("worktree")).unwrap(),
            root.display().to_string()
        );
        let at = |flag: &str| spec.args.iter().position(|a| a == flag).unwrap();
        assert_eq!(spec.args[at("-jar") + 1], launcher.display().to_string());
        assert_eq!(spec.args[at("-data") + 1], data.display().to_string());
        assert_eq!(
            spec.args[at("-configuration") + 1],
            into.join(jdtls_config(std::env::consts::OS, std::env::consts::ARCH))
                .display()
                .to_string()
        );
        assert_eq!(
            spec.options["extendedClientCapabilities"]["classFileContentsSupport"],
            true
        );
        assert_eq!(
            spec.options["settings"]["java"]["import"]["generatesMetadataFilesAtProjectRoot"],
            false
        );
        assert_ne!(jdtls_data(&fake.env(), &fake.path("elsewhere")), data);

        let wrapper = fake.tool("jdtls", "exit 0");
        let spec = jdtls(&fake).unwrap();
        assert_eq!(spec.binary, wrapper);
        assert_eq!(
            spec.args,
            [
                format!("--jvm-arg={NO_METADATA_AT_ROOT}"),
                "-data".to_string(),
                data.display().to_string()
            ]
        );
    }

    /// Off macOS a JDK is `JAVA_HOME`, else the one `java` on `PATH` links
    /// into, else the newest installed; one older than 21 is passed over.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn a_jdk_is_java_home_then_javas_on_path_then_the_newest_installed() {
        let fake = Fake::new();
        assert_eq!(find_jdk(&fake.env()), None);

        let old = fake.jdk("jvm/java-17-openjdk", "17.0.9");
        assert_eq!(find_jdk(&fake.env()), None, "17 is too old");
        let newer = fake.jdk("jvm/java-26-openjdk", "26.0.1");
        let new = fake.jdk("jvm/java-21-openjdk", "21.0.12");
        // As Arch has it, a `default` link beside the JDKs.
        std::os::unix::fs::symlink(&new, fake.path("jvm/default")).unwrap();
        assert_eq!(
            find_jdk(&fake.env()),
            Some(newer.clone()),
            "the newest installed"
        );

        // `java` on PATH is the system's choice, through a link.
        std::os::unix::fs::symlink(new.join("bin/java"), fake.path("bin/java")).unwrap();
        assert_eq!(find_jdk(&fake.env()), Some(new.clone()));

        let env = Env {
            java_home: Some(newer.clone()),
            ..fake.env()
        };
        assert_eq!(find_jdk(&env), Some(newer.clone()), "JAVA_HOME first");
        let env = Env {
            java_home: Some(old.clone()),
            ..fake.env()
        };
        assert_eq!(find_jdk(&env), Some(new), "an old JAVA_HOME is passed over");
    }

    /// Each platform's own launcher configuration, as the download names
    /// them.
    #[test]
    fn jdtls_is_configured_for_the_platform_it_runs_on() {
        assert_eq!(jdtls_config("macos", "aarch64"), "config_mac_arm");
        assert_eq!(jdtls_config("macos", "x86_64"), "config_mac");
        assert_eq!(jdtls_config("linux", "x86_64"), "config_linux");
        assert_eq!(jdtls_config("linux", "aarch64"), "config_linux_arm");
        assert_eq!(jdtls_config("windows", "x86_64"), "config_win");
    }

    #[test]
    fn java_says_its_major_version() {
        assert_eq!(
            java_major("openjdk version \"21.0.2\" 2024-01-16"),
            Some(21)
        );
        assert_eq!(java_major("java version \"1.8.0_402\""), Some(8));
        assert_eq!(java_major("openjdk version \"25\" 2025-09-16"), Some(25));
        assert_eq!(java_major("nothing"), None);
        assert_eq!(java_major("JAVA_VERSION=\"21.0.12\""), Some(21));
        assert!(is_class_file("jdt://contents/rt.jar/java.util/List.class"));
        assert_eq!(
            language_id("jdt://contents/rt.jar/java.util/List.class"),
            Some((ServerKind::Jdtls, "java"))
        );
    }

    /// Only Perch's own install, older than the pin, is offered an update.
    #[test]
    fn an_old_install_of_perchs_is_offered_the_pin() {
        let fake = Fake::new();
        fake.tool("node", "exit 0");
        let npm = fake.tool("npm", "exit 0");
        let kind = ServerKind::TypescriptLanguageServer;
        fake.perchs_typescript_server("5.1.0");
        let old = fake.path("servers/typescript-language-server/5.1.0");
        let spec = find(kind, &fake.path("checkout"), &fake.env()).unwrap();
        let old_one = outdated(&spec, &fake.env()).unwrap();
        assert_eq!(old_one.installed, "5.1.0");
        assert_eq!(old_one.pin, "6.0.1");
        assert_eq!(
            old_one.describe(),
            "typescript-language-server 6.0.1 is available."
        );
        assert_eq!(old_one.install, typescript_install(&npm, &fake.env()));

        // Up to date: nothing.
        std::fs::rename(&old, fake.path("servers/typescript-language-server/6.0.1")).unwrap();
        let spec = find(kind, &fake.path("checkout"), &fake.env()).unwrap();
        assert_eq!(outdated(&spec, &fake.env()), None);
        // The user's own: never Perch's to update.
        fake.global_typescript_server("5.1.3");
        fake.typescript("global/lib/node_modules/typescript", TYPESCRIPT_PACKAGE);
        std::fs::rename(
            fake.path("servers/typescript-language-server/6.0.1"),
            fake.path("servers/typescript-language-server/5.0.0"),
        )
        .unwrap();
        let spec = find(kind, &fake.path("checkout"), &fake.env()).unwrap();
        assert_eq!(outdated(&spec, &fake.env()), None);
    }

    /// Startup leaves the newest install of each server, finished, and the
    /// jdtls data of checkouts still there.
    #[test]
    fn the_sweep_keeps_the_newest_finished_install_and_live_data() {
        let fake = Fake::new();
        let servers = fake.path("servers");
        for version in ["0.21.1", "0.23.0", "0.24.0.partial"] {
            std::fs::create_dir_all(servers.join("gopls").join(version)).unwrap();
        }
        std::fs::create_dir_all(servers.join("jdtls/1.61.0.partial")).unwrap();
        for version in ["0.0.85", "0.0.86", "0.0.87.partial"] {
            std::fs::create_dir_all(servers.join("ty").join(version)).unwrap();
        }
        let live = jdtls_data(&fake.env(), &fake.path("checkout"));
        let gone = jdtls_data(&fake.env(), &fake.path("gone"));
        let unmarked = servers.join("jdtls-data/000000000000");
        std::fs::create_dir_all(&unmarked).unwrap();

        sweep(&servers);
        let left = |dir: &str| {
            let mut names: Vec<String> = std::fs::read_dir(servers.join(dir))
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        assert_eq!(left("gopls"), ["0.23.0"]);
        assert_eq!(left("ty"), ["0.0.86"]);
        assert!(left("jdtls").is_empty());
        assert!(live.is_dir());
        assert!(!gone.exists());
        assert!(!unmarked.exists());
    }

    #[test]
    fn go_says_its_version() {
        assert_eq!(
            parse_go_version("go version go1.25.0 darwin/arm64\n"),
            Some((1, 25))
        );
        assert_eq!(
            parse_go_version("go version go1.26rc1 linux/amd64"),
            Some((1, 26))
        );
        assert_eq!(parse_go_version("nonsense"), None);
    }

    #[test]
    fn versions_compare_number_by_number() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("0.23.0", "0.9.1"), Ordering::Greater);
        assert_eq!(compare_versions("v1.61.0", "1.60.0"), Ordering::Greater);
        assert_eq!(compare_versions("6.0.1", "6.0.1"), Ordering::Equal);
    }

    #[test]
    fn without_rustup_rust_analyzer_needs_it() {
        let fake = Fake::new();
        let missing = fake.rust().unwrap_err();
        assert_eq!(missing.needs, Some(Need::Rustup));
        assert_eq!(missing.install, None);
        assert_eq!(
            missing.describe(),
            "rust-analyzer needs rustup, which isn't on your PATH."
        );
        fake.tool("rust-analyzer", "exit 0");
        assert!(fake.rust().is_ok());
    }

    #[test]
    fn a_file_picks_its_server_by_extension() {
        assert_eq!(
            language_id("src/lib.rs"),
            Some((ServerKind::RustAnalyzer, "rust"))
        );
        assert_eq!(
            language_id("/abs/MAIN.RS"),
            Some((ServerKind::RustAnalyzer, "rust"))
        );
        assert_eq!(ServerKind::for_path("README.md"), None);
        assert_eq!(ServerKind::for_path("Makefile"), None);
        assert_eq!(
            ServerKind::for_fence("Rust"),
            Some(ServerKind::RustAnalyzer)
        );
        assert_eq!(ServerKind::for_fence("diff"), None);
        assert_eq!(language_id("cmd/main.go"), Some((ServerKind::Gopls, "go")));
        assert_eq!(ServerKind::for_fence("golang"), Some(ServerKind::Gopls));
        let ts = ServerKind::TypescriptLanguageServer;
        assert_eq!(language_id("src/App.tsx"), Some((ts, "typescriptreact")));
        assert_eq!(language_id("src/App.jsx"), Some((ts, "javascriptreact")));
        assert_eq!(language_id("index.mjs"), Some((ts, "javascript")));
        assert_eq!(language_id("a.d.cts"), Some((ts, "typescript")));
        assert_eq!(ServerKind::for_fence("tsx"), Some(ts));
        let ty = ServerKind::Ty;
        assert_eq!(language_id("pkg/app.py"), Some((ty, "python")));
        assert_eq!(language_id("stubs/app.pyi"), Some((ty, "python")));
        assert_eq!(language_id("bin/run.PYW"), Some((ty, "python")));
        assert_eq!(ServerKind::for_fence("py"), Some(ty));
        assert_eq!(ServerKind::for_fence("Python"), Some(ty));
    }

    #[test]
    fn a_root_marker_names_its_kind() {
        let dir = std::env::temp_dir().join(format!("perch-lsp-markers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(kinds_at_root(&dir).is_empty());
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        assert_eq!(kinds_at_root(&dir), [ServerKind::RustAnalyzer]);
        std::fs::write(dir.join("go.work"), "").unwrap();
        assert_eq!(
            kinds_at_root(&dir),
            [ServerKind::RustAnalyzer, ServerKind::Gopls]
        );
        std::fs::write(dir.join("pyproject.toml"), "").unwrap();
        assert_eq!(
            kinds_at_root(&dir),
            [ServerKind::RustAnalyzer, ServerKind::Gopls, ServerKind::Ty]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_rust_analyzer_and_jdtls_run_a_checkout_s_code() {
        assert_eq!(
            ServerKind::RustAnalyzer.project_code(),
            Some("build scripts and proc macros")
        );
        assert_eq!(
            ServerKind::Jdtls.project_code(),
            Some("Gradle or Maven build")
        );
        assert_eq!(ServerKind::Gopls.project_code(), None);
        assert_eq!(ServerKind::TypescriptLanguageServer.project_code(), None);
        assert_eq!(ServerKind::Ty.project_code(), None);
    }

    #[test]
    fn status_text_is_written_for_a_status_line() {
        let rust = ServerKind::RustAnalyzer;
        assert_eq!(Status::Ready.describe(rust), "rust-analyzer ready");
        assert_eq!(
            Status::Indexing { percent: Some(42) }.describe(rust),
            "rust-analyzer indexing 42%"
        );
        assert!(Status::Failed {
            reason: "boom".into()
        }
        .describe(rust)
        .contains("boom"));
        assert_eq!(
            Status::Unhealthy {
                message: "Failed to load workspaces\n\nUsage: cargo metadata".into()
            }
            .describe(rust),
            "rust-analyzer: Failed to load workspaces"
        );
        assert_eq!(
            Status::Held.describe(rust),
            "rust-analyzer waits for you to trust this code"
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
        assert!(!Status::Unhealthy {
            message: "no workspace".into()
        }
        .is_usable());
        assert!(!Status::Held.is_usable());
    }

    #[test]
    fn a_server_is_loading_until_it_is_ready_or_broken() {
        assert!(Status::Starting.is_loading());
        assert!(Status::Indexing { percent: None }.is_loading());
        assert!(!Status::Ready.is_loading());
        assert!(!Status::Unhealthy {
            message: "no workspace".into()
        }
        .is_loading());
        assert!(!Status::Failed {
            reason: "exited".into()
        }
        .is_loading());
        // Held never loads: it waits on the reader, not the server.
        assert!(!Status::Held.is_loading());
    }
}
