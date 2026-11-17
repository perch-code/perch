//! Getting the app's `perchd` to a host. With no `[remote] command`, the
//! app runs its own engine there: the file is kept by what it is,
//! `${PERCH_HOME:-$HOME/.perch}/bin/perchd-<build>`, and when the host has
//! none of that build it says so on stderr and the link sends it, with a
//! second ssh, and dials again.
//!
//! - `<build>` is the first 12 hex digits of the binary's SHA-256,
//!   whatever its version, so the same engine is never sent twice and
//!   another one never takes its name. The binary is hashed, not the file:
//!   a tarball and the binary in it are the same build.
//! - Every app carries its engines in itself: its build builds `perchd`
//!   for each system and processor Perch runs on and puts them, gzipped,
//!   in the app's own binary, with their hashes. Nothing is downloaded,
//!   and nothing is read from beside the app.
//! - The host takes the upload's whole SHA-256 before it moves the file
//!   into place, so one cut short or changed on the way is never run.
//! - What ssh runs on the host goes through the reader's login shell,
//!   which may be bash, zsh or fish, so the engine gets the `PATH` their
//!   profile sets. The logic is a POSIX `sh` script, and the quoting around
//!   it holds no single quotes and no backslash a shell would read as one.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::backend::InstallProgress;

pub type Progress = tokio::sync::watch::Sender<Option<InstallProgress>>;

pub fn phase(progress: &Progress, name: &str) {
    progress.send_replace(Some(InstallProgress::phase(name)));
}

/// The start of what the host says on stderr when it has no `perchd` of
/// the build asked for, before `<uname -s> <uname -m>`.
pub const INSTALL_SAID: &str = "perch-install:";

/// How the connection script exits when it says `INSTALL_SAID`.
pub const INSTALL_EXIT: i32 = 97;

/// The start of what `perchd bridge --replace-if-idle` says on stderr when
/// the engine it would replace has runs or clients, before `<version>
/// <runs> <clients>`.
pub const BUSY_SAID: &str = "perch-busy:";

/// How `perchd bridge --replace-if-idle` exits when it says `BUSY_SAID`.
pub const BUSY_EXIT: u8 = 98;

/// What the bridge would do about an engine of another build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Replace {
    /// Join whatever engine is there.
    #[default]
    No,
    /// `--replace-if-idle`: stop it first unless it has runs or clients.
    IfIdle,
    /// `--replace`: stop it first.
    Always,
}

impl Replace {
    /// The bridge's flag for it.
    pub fn flag(self) -> Option<&'static str> {
        match self {
            Replace::No => None,
            Replace::IfIdle => Some("--replace-if-idle"),
            Replace::Always => Some("--replace"),
        }
    }
}

/// An engine `--replace-if-idle` left alone, as its line says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    pub version: String,
    pub runs: usize,
    pub clients: usize,
}

/// What a line the host said means when it is the bridge saying the
/// engine is busy.
pub fn busy_line(line: &str) -> Option<Busy> {
    let rest = line.trim().strip_prefix(BUSY_SAID)?;
    let mut words = rest.split_whitespace();
    Some(Busy {
        version: words.next()?.to_string(),
        runs: words.next()?.parse().ok()?,
        clients: words.next()?.parse().ok()?,
    })
}

/// The largest `perchd` taken: a release's is around 20 MB, and one
/// built unoptimized, with its line tables, several times that.
const MAX_BINARY: u64 = 256 << 20;

/// An engine the app carries in itself: a `perchd` for one platform, as
/// the app's build put it there.
#[derive(Clone)]
pub struct Carried {
    pub platform: Platform,
    /// The binary's whole SHA-256, in hex, taken when the app was built.
    pub sum: String,
    /// The binary, gzipped.
    pub packed: std::borrow::Cow<'static, [u8]>,
}

impl Carried {
    /// What the app's build embedded for the platform `key` names, or
    /// None when it names none Perch knows, or `sum` is no SHA-256.
    pub fn embedded(key: &str, sum: &str, packed: &'static [u8]) -> Option<Carried> {
        let whole = sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit());
        Some(Carried {
            platform: Platform::from_key(key).filter(|_| whole)?,
            sum: sum.to_ascii_lowercase(),
            packed: packed.into(),
        })
    }

    /// `binary`, packed as an app's build packs it: for a test, which
    /// has no build to do it.
    pub fn pack(platform: Platform, binary: &[u8]) -> Carried {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let packed = encoder
            .write_all(binary)
            .and_then(|()| encoder.finish())
            .unwrap_or_default();
        Carried {
            platform,
            sum: sum_of(binary),
            packed: packed.into(),
        }
    }

    /// Its build: the start of its SHA-256.
    pub fn build(&self) -> &str {
        &self.sum[..BUILD_DIGITS.min(self.sum.len())]
    }

    /// The binary, unpacked and checked against the hash it was embedded
    /// with and the platform it is for.
    pub fn binary(&self) -> Result<Binary, Failed> {
        use std::io::Read;
        let wrong = |why: String| {
            Failed::lasting(format!(
                "The perchd this Perch carries for {} can't be used: {why}. Build Perch again.",
                self.platform.name()
            ))
        };
        let mut bytes = Vec::new();
        flate2::read::GzDecoder::new(&self.packed[..])
            .take(MAX_BINARY + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| wrong(format!("it does not unpack ({e})")))?;
        if bytes.len() as u64 > MAX_BINARY {
            return Err(wrong("it is larger than any perchd".to_string()));
        }
        let binary = Binary::of(bytes);
        if binary.sum != self.sum {
            return Err(wrong("it is not the binary it was built with".to_string()));
        }
        match built_for(&binary.bytes) {
            Ok(built) if built.contains(&self.platform) => Ok(binary),
            Ok(_) => Err(wrong("it is built for another platform".to_string())),
            Err(why) => Err(wrong(why)),
        }
    }
}

impl std::fmt::Debug for Carried {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.platform.key(), self.build())
    }
}

/// By what they are, not by how they are packed.
impl PartialEq for Carried {
    fn eq(&self, other: &Self) -> bool {
        self.platform == other.platform && self.sum == other.sum
    }
}

impl Eq for Carried {}

/// Where the app's own `perchd` comes from, for whatever a host turns
/// out to be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Source {
    /// The engines the app carries, one to a platform. A build made
    /// without the tools for one carries the rest.
    pub carried: Vec<Carried>,
    /// The workspace the app was built from, when it is still there: for
    /// a host the app carries no engine for, `perchd` is built from the
    /// same source, for what the host says it is, and uploaded.
    pub workspace: Option<PathBuf>,
}

impl Source {
    /// The engine carried for `platform`.
    pub fn carried_for(&self, platform: Platform) -> Option<&Carried> {
        self.carried
            .iter()
            .find(|engine| engine.platform == platform)
    }
}

/// Why nothing can be sent to a host that is `platform`, by an app that
/// carries no engine for it and has no source to build one from.
pub fn none_carried(platform: Platform) -> String {
    format!(
        "This Perch carries no perchd for {}: it was built without what building one needs. Install Perch from a release, which carries every one, or set [remote] command to run a perchd put on the host by hand.",
        platform.name()
    )
}

/// A host's processor, as Perch's releases name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Arm64,
}

impl Arch {
    pub fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
        }
    }

    /// Its number in an ELF header's `e_machine`.
    fn elf_machine(self) -> u16 {
        match self {
            Arch::X86_64 => 62,
            Arch::Arm64 => 183,
        }
    }

    /// Its number in a Mach-O header's `cputype`.
    fn mach_cpu(self) -> u32 {
        match self {
            Arch::X86_64 => 0x0100_0007,
            Arch::Arm64 => 0x0100_000c,
        }
    }
}

/// A host's system: the ones Perch builds `perchd` for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
}

/// What a host is, as far as which `perchd` runs there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
}

impl Platform {
    pub const fn new(os: Os, arch: Arch) -> Self {
        Platform { os, arch }
    }

    /// As a reader would say it: "Linux on x86_64".
    pub fn name(self) -> String {
        let os = match self.os {
            Os::Linux => "Linux",
            Os::MacOs => "macOS",
        };
        format!("{os} on {}", self.arch.name())
    }

    /// The platform `key` names, as [`Platform::key`] writes it.
    pub fn from_key(key: &str) -> Option<Platform> {
        Platform::ALL
            .into_iter()
            .find(|platform| platform.key() == key)
    }

    /// As the releases and the connection script name it: `linux-x86_64`.
    pub fn key(self) -> String {
        let os = match self.os {
            Os::Linux => "linux",
            Os::MacOs => "macos",
        };
        format!("{os}-{}", self.arch.name())
    }

    /// The release's tarball for it.
    pub fn asset(self) -> String {
        format!("perchd-{}.tar.gz", self.key())
    }

    /// Every one Perch builds `perchd` for.
    pub const ALL: [Platform; 4] = [
        Platform::new(Os::Linux, Arch::X86_64),
        Platform::new(Os::Linux, Arch::Arm64),
        Platform::new(Os::MacOs, Arch::X86_64),
        Platform::new(Os::MacOs, Arch::Arm64),
    ];
}

impl Platform {
    /// What this machine is, when it is one Perch builds `perchd` for.
    pub fn here() -> Option<Platform> {
        let os = match std::env::consts::OS {
            "linux" => Os::Linux,
            "macos" => Os::MacOs,
            _ => return None,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => Arch::X86_64,
            "aarch64" => Arch::Arm64,
            _ => return None,
        };
        Some(Platform::new(os, arch))
    }
}

/// How `perchd` is built from source for a host: cargo's subcommand, and
/// the target it is told, none for this machine's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevBuild {
    /// `build`, or `zigbuild` where the host's system is not this one's:
    /// a Linux `perchd` from a Mac needs a C compiler and a linker for
    /// Linux, which zig is.
    pub subcommand: &'static str,
    pub triple: Option<&'static str>,
}

/// How to build `perchd` here for `platform`.
///
/// A Linux host's is always against musl and through zig, as the
/// release's is static: this machine's own build would need this
/// machine's libc there, or newer, and another processor's has no C
/// compiler and linker here without zig. A Mac's is the toolchain's own
/// from a Mac, for either processor, and zig's from anywhere else.
pub fn dev_build_for(platform: Platform, here: Option<Platform>) -> DevBuild {
    let from_a_mac = here.is_some_and(|here| here.os == Os::MacOs);
    match (platform.os, platform.arch) {
        (Os::MacOs, _) if here == Some(platform) => DevBuild {
            subcommand: "build",
            triple: None,
        },
        (Os::MacOs, arch) => DevBuild {
            subcommand: if from_a_mac { "build" } else { "zigbuild" },
            triple: Some(match arch {
                Arch::X86_64 => "x86_64-apple-darwin",
                Arch::Arm64 => "aarch64-apple-darwin",
            }),
        },
        (Os::Linux, arch) => DevBuild {
            subcommand: "zigbuild",
            triple: Some(match arch {
                Arch::X86_64 => "x86_64-unknown-linux-musl",
                Arch::Arm64 => "aarch64-unknown-linux-musl",
            }),
        },
    }
}

/// The `perchd` cargo says it built, in what `--message-format=json`
/// printed: the last artifact named `perchd` with an executable. Cargo
/// says where, whatever `CARGO_TARGET_DIR`, `build.target-dir` or
/// `build.target` moved it to.
pub fn built_perchd(messages: &str) -> Option<PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| {
            message["reason"] == "compiler-artifact" && message["target"]["name"] == "perchd"
        })
        .filter_map(|message| message["executable"].as_str().map(PathBuf::from))
        .last()
}

/// `PATH` for cargo, with cargo's own bin directory after what is there:
/// an app started from the Finder or a debugger has a bare one, and
/// cargo-zigbuild is found by it too.
fn path_with_cargo() -> std::ffi::OsString {
    let cargo_bin = match std::env::var_os("CARGO_HOME").filter(|home| !home.is_empty()) {
        Some(home) => PathBuf::from(home).join("bin"),
        None => perch_github::config::home_dir().join(".cargo/bin"),
    };
    let dirs = std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .chain([cargo_bin])
        .collect::<Vec<_>>();
    std::env::join_paths(dirs).unwrap_or_default()
}

/// What a failed build means for the reader: the tool to install when
/// that is what cargo said was missing, else cargo's last error.
pub fn dev_build_failure(build: DevBuild, platform: Platform, said: &str) -> String {
    let what = format!("Couldn't build perchd for {}", platform.name());
    if said.contains("no such command: `zigbuild`")
        || said.contains("no such subcommand: `zigbuild`")
    {
        return format!(
            "{what}: building it from here needs zig and cargo-zigbuild. Install them with: brew install zig && cargo install cargo-zigbuild"
        );
    }
    if said.contains("zig") && (said.contains("not found") || said.contains("No such file")) {
        return format!("{what}: cargo-zigbuild found no zig. Install it with: brew install zig");
    }
    if let Some(triple) = build.triple {
        if said.contains("target may not be installed")
            || said.contains("can't find crate for `core`")
        {
            return format!(
                "{what}: the Rust target is not installed. Install it with: rustup target add {triple}"
            );
        }
    }
    let last = said
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("error"))
        .or_else(|| {
            said.lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
        })
        .unwrap_or("cargo said nothing");
    format!("{what}: {last}")
}

/// Build `perchd` from the source in `workspace` for `platform`, and give
/// where it is. Cargo's own incremental build makes this quick when
/// nothing changed.
pub async fn dev_build(workspace: &Path, platform: Platform) -> Result<PathBuf, Failed> {
    let build = dev_build_for(platform, Platform::here());
    let mut command = tokio::process::Command::new("cargo");
    command.current_dir(workspace).args([
        build.subcommand,
        "-p",
        "perchd",
        "--message-format=json",
    ]);
    if let Some(triple) = build.triple {
        command.args(["--target", triple]);
    }
    command
        .env("PATH", path_with_cargo())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    tracing::info!(
        platform = platform.name(),
        subcommand = build.subcommand,
        triple = build.triple,
        "building perchd from source"
    );
    let output = tokio::time::timeout(std::time::Duration::from_secs(1800), command.output())
        .await
        .map_err(|_| {
            Failed::now(format!(
                "Building perchd for {} took over half an hour.",
                platform.name()
            ))
        })?
        .map_err(|e| Failed::lasting(format!("Could not run cargo to build perchd: {e}")))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        for line in said.lines().rev().take(20).collect::<Vec<_>>().iter().rev() {
            tracing::warn!("cargo: {line}");
        }
        return Err(Failed::lasting(dev_build_failure(build, platform, &said)));
    }
    built_perchd(&String::from_utf8_lossy(&output.stdout))
        .filter(|binary| binary.is_file())
        .ok_or_else(|| {
            Failed::lasting(format!(
                "Cargo built perchd for {} and did not say where.",
                platform.name()
            ))
        })
}

/// The host `uname -s` and `uname -m` name, or why Perch has no `perchd`
/// for it.
pub fn platform(os: &str, machine: &str) -> Result<Platform, String> {
    let arch = match machine {
        "x86_64" | "amd64" => Some(Arch::X86_64),
        "aarch64" | "arm64" => Some(Arch::Arm64),
        _ => None,
    };
    let system = match os {
        "Linux" => Some(Os::Linux),
        "Darwin" => Some(Os::MacOs),
        _ => None,
    };
    match (system, arch) {
        (Some(os), Some(arch)) => Ok(Platform::new(os, arch)),
        _ => Err(format!(
            "The host is {os} on {machine}, and Perch has perchd only for Linux and macOS, on x86_64 and arm64. Install perchd there by hand and set [remote] command to run it."
        )),
    }
}

/// What a line the host said asks for: None when it is not the line
/// saying `perchd` is missing, else what to install for, or why there is
/// none.
pub fn install_line(line: &str) -> Option<Result<Platform, String>> {
    let rest = line.trim().strip_prefix(INSTALL_SAID)?;
    let mut words = rest.split_whitespace();
    let (os, machine) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    Some(platform(os, machine))
}

/// How many hex digits of a binary's SHA-256 name its build.
pub const BUILD_DIGITS: usize = 12;

/// The SHA-256 of `binary`, in hex.
pub fn sum_of(binary: &[u8]) -> String {
    Sha256::digest(binary)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The build of `binary`: the start of its SHA-256.
pub fn build_of(binary: &[u8]) -> String {
    sum_of(binary)[..BUILD_DIGITS].to_string()
}

/// A platform word of the connection script's that every host answers
/// to: for a `perchd` built for the host, after it said what it is.
pub const ANY: &str = "any";

/// Whether `build` can go on a command line unquoted in every shell, and
/// in a file name: a version's characters only.
pub fn plain_word(build: &str) -> bool {
    !build.is_empty()
        && build
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

/// Whether `path` can go on a command line unquoted in every shell: a
/// build's characters and `/`.
fn plain_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-/".contains(&b))
}

/// The connection script, run by `sh` with the bridge's flag as `$1`, `-`
/// for none, and after it the builds the app has, each as two words: the
/// platform it is for, `linux-x86_64` say or `any`, and the build. The
/// host picks the one for what it is and runs its bridge if it is there.
/// Else it says what it is, for the app to send the engine for.
const CONNECT: &str = r#"d="${PERCH_HOME:-$HOME/.perch}/bin"
case "$(uname -s)" in
  Linux) s=linux ;;
  Darwin) s=macos ;;
  *) s=other ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) m=x86_64 ;;
  aarch64 | arm64) m=arm64 ;;
  *) m=other ;;
esac
f="$1"
shift
b=
while [ $# -gt 1 ]; do
  if [ "$1" = "$s-$m" ] || [ "$1" = any ]; then
    b="$2"
  fi
  shift 2
done
if [ -n "$b" ] && [ -x "$d/perchd-$b" ]; then
  case "$f" in
    --*) exec "$d/perchd-$b" bridge "$f" ;;
    *) exec "$d/perchd-$b" bridge ;;
  esac
fi
echo "perch-install: $(uname -s) $(uname -m)" >&2
exit 97"#;

/// The upload script, run by `sh` with the build and the binary's whole
/// SHA-256 as `$1` and `$2`, and the binary on stdin. It writes a
/// temporary file beside the others, owner-only, takes its SHA-256, and
/// puts it in place only when that is the one expected: an upload cut
/// short, or changed on the way, is never run. The file is read on the
/// hashing tool's stdin, not named to it: named, a path with a backslash
/// in it is written back escaped, behind a backslash of its own.
///
/// It is put in place by a link, which fails where a file is already:
/// two apps of one build sending it at once then leave the first to
/// arrive, the same bytes, where a move would put another file under the
/// name a bridge is already running from. The temporary file goes
/// whatever fails, the ssh session ending under it included: a write to
/// its closed stderr is a SIGPIPE, which the trap turns into an exit, so
/// the file goes with it. One left by a script killed outright is the
/// bridge's to clear.
const UPLOAD: &str = r#"d="${PERCH_HOME:-$HOME/.perch}/bin"
umask 077
mkdir -p "$d" && chmod 700 "$d" || {
  echo "perch-upload: could not make $d" >&2
  exit 1
}
t="$d/.perchd-$1.$$"
gone() {
  rm -f "$t"
}
trap gone EXIT
trap "exit 1" HUP INT PIPE TERM
cat > "$t" && chmod 700 "$t" || {
  echo "perch-upload: could not write $t" >&2
  exit 1
}
h=$({ sha256sum < "$t" 2>/dev/null || shasum -a 256 < "$t" 2>/dev/null || openssl dgst -sha256 -r < "$t"; } | cut -c1-64)
if [ "$h" = "$2" ]; then
  :
else
  echo "perch-sum: $h" >&2
  exit 1
fi
if ln "$t" "$d/perchd-$1" 2>/dev/null || [ -x "$d/perchd-$1" ]; then
  :
else
  rm -f "$d/perchd-$1"
  ln "$t" "$d/perchd-$1" || {
    echo "perch-upload: could not put it at $d/perchd-$1" >&2
    exit 1
  }
fi
echo "perch-installed: $d/perchd-$1""#;

/// Whether the `perchd` of the build given as `$1` is on the host, by how
/// it exits: asked before an upload that would only replace it with
/// itself. One that is there is touched, as one in use.
const HAS: &str = r#"d="${PERCH_HOME:-$HOME/.perch}/bin"
if [ -f "$d/perchd-$1" ] && [ -x "$d/perchd-$1" ]; then
  touch "$d/perchd-$1" 2>/dev/null || :
  echo "perch-has: $1"
  exit 0
fi
exit 1"#;

/// What ssh is given to run `script` with `args` on the host: the login
/// shell, whichever it is, runs `/bin/sh -c "<script>" <name> <args>`.
///
/// The outer quotes are single, so the shell ssh starts passes the inner
/// command through untouched; neither layer has a single quote inside.
/// The script is in double quotes for the login shell, with each `$` and
/// `"` behind a backslash, which bash, zsh and fish all read as the
/// character itself. A script with a single quote, a backslash, a
/// backquote or a `!` could be read differently by one of them, so none
/// has one.
fn in_login_sh(name: &str, script: &str, args: &[&str]) -> String {
    debug_assert!(
        !script.contains(['\'', '\\', '`', '!']),
        "a script for every login shell"
    );
    debug_assert!(args.iter().all(|arg| plain_path(arg)));
    let script = script.replace('"', "\\\"").replace('$', "\\$");
    let mut inner = format!("exec /bin/sh -c \"{script}\" {name}");
    for arg in args {
        inner.push(' ');
        inner.push_str(arg);
    }
    format!("exec \"$SHELL\" -lc '{inner}'")
}

/// What ssh runs on the host to reach the engine through the `perchd`
/// the app has for it, of `builds`, each a platform's key (or [`ANY`])
/// and the build for it, replacing an engine of another build as
/// `replace` says; or to say the host has none of them.
pub fn connect_command(builds: &[(String, String)], replace: Replace) -> String {
    let mut args = vec![replace.flag().unwrap_or("-")];
    for (platform, build) in builds {
        args.extend([platform.as_str(), build.as_str()]);
    }
    in_login_sh("perch-connect", CONNECT, &args)
}

/// What ssh runs on the host to put the `perchd` of `build`, whose whole
/// SHA-256 is `sum`, in place from its stdin.
pub fn upload_command(build: &str, sum: &str) -> String {
    in_login_sh("perch-upload", UPLOAD, &[build, sum])
}

/// What ssh runs on the host to say whether the `perchd` of `build` is
/// installed.
pub fn has_command(build: &str) -> String {
    in_login_sh("perch-has", HAS, &[build])
}

/// What a failed upload's last line says, put for the reader.
pub fn upload_failure(line: &str, host: &str) -> String {
    if let Some(said) = line.trim().strip_prefix("perch-sum:") {
        return if said.trim().is_empty() {
            format!(
                "{host} has no sha256sum, shasum or openssl to check the perchd sent to it with, so it was not kept."
            )
        } else {
            format!("The perchd sent to {host} arrived cut short or changed, so it was not kept.")
        };
    }
    match line.trim().strip_prefix("perch-upload:") {
        Some(said) => format!("Couldn't put perchd on {host}: {}.", said.trim()),
        None => format!("Couldn't put perchd on {host}: {}", line.trim()),
    }
}

/// What a binary runs on, or why it is no `perchd` of Perch's: a Linux
/// binary's one processor, a macOS one's, or each of a universal macOS
/// binary's.
pub fn built_for(binary: &[u8]) -> Result<Vec<Platform>, String> {
    let both = [Arch::X86_64, Arch::Arm64];
    let word = |at: usize, big: bool| {
        binary.get(at..at + 4).map(|bytes| {
            let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
            if big {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            }
        })
    };
    let mac = |cpu: u32| {
        both.into_iter()
            .find(|arch| arch.mach_cpu() == cpu)
            .map(|arch| Platform::new(Os::MacOs, arch))
    };
    let neither = |what: String| format!("it is a binary for neither x86_64 nor arm64 ({what})");
    match binary.get(..4) {
        Some(b"\x7fELF") if binary.len() >= 20 => {
            let machine = u16::from_le_bytes([binary[18], binary[19]]);
            both.into_iter()
                .find(|arch| arch.elf_machine() == machine)
                .map(|arch| vec![Platform::new(Os::Linux, arch)])
                .ok_or_else(|| neither(format!("machine {machine}")))
        }
        // A 64-bit Mach-O, as either processor writes it.
        Some([0xcf, 0xfa, 0xed, 0xfe]) => {
            let cpu = word(4, false).unwrap_or(0);
            mac(cpu)
                .map(|platform| vec![platform])
                .ok_or_else(|| neither(format!("cpu {cpu:#x}")))
        }
        // A universal one: a count, then twenty bytes for each.
        Some([0xca, 0xfe, 0xba, 0xbe]) => {
            let count = word(4, true).unwrap_or(0).min(8) as usize;
            let built: Vec<Platform> = (0..count)
                .filter_map(|n| word(8 + 20 * n, true))
                .filter_map(mac)
                .collect();
            if built.is_empty() {
                Err(neither("a universal binary with neither in it".to_string()))
            } else {
                Ok(built)
            }
        }
        _ => Err("it is neither a Linux nor a macOS binary".to_string()),
    }
}

/// A `perchd` to send, its build, and the whole SHA-256 the build is the
/// start of.
#[derive(Debug, Clone)]
pub struct Binary {
    pub build: String,
    pub sum: String,
    pub bytes: Vec<u8>,
}

impl Binary {
    fn of(bytes: Vec<u8>) -> Binary {
        let sum = sum_of(&bytes);
        Binary {
            build: sum[..BUILD_DIGITS].to_string(),
            sum,
            bytes,
        }
    }
}

/// The `perchd` at `path`, which the reader knows as `what`.
pub fn read_binary(path: &Path, what: &str) -> Result<Binary, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Couldn't read {what}, {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BINARY {
        return Err(format!(
            "{what}, {}, is larger than any perchd.",
            path.display()
        ));
    }
    built_for(&bytes)
        .map_err(|why| format!("{what}, {}, can't be used: {why}.", path.display()))?;
    Ok(Binary::of(bytes))
}

/// Why an install did not happen, and whether trying again could help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub why: String,
    /// Trying again cannot help until the reader changes something: an
    /// app that carries no `perchd` for the host, a host Perch has none
    /// for.
    pub lasting: bool,
}

impl Failed {
    pub fn now(why: String) -> Self {
        Failed {
            why,
            lasting: false,
        }
    }

    pub fn lasting(why: String) -> Self {
        Failed { why, lasting: true }
    }
}

/// Remove what an app from before engines were carried kept in `dir`,
/// its Perch home's `perchd` folder: each release's `perchd` tarballs and
/// `SHA256SUMS`, downloaded into a folder named for the version. Nothing
/// reads them now. Only those files go, and then the folders they leave
/// empty, so anything else someone put there stays, folder and all.
pub fn clear_old_downloads(dir: &Path) {
    let Ok(versions) = std::fs::read_dir(dir) else {
        return;
    };
    for version in versions.flatten().map(|entry| entry.path()) {
        for file in std::fs::read_dir(&version).into_iter().flatten().flatten() {
            let name = file.file_name();
            let name = name.to_string_lossy();
            let downloaded = name == "SHA256SUMS"
                || name == "SHA256SUMS.part"
                || (name.starts_with("perchd-")
                    && (name.ends_with(".tar.gz") || name.ends_with(".tar.part")));
            if downloaded && file.file_type().is_ok_and(|kind| kind.is_file()) {
                let _ = std::fs::remove_file(file.path());
            }
        }
        // Fails, as it should, on one that is not empty.
        let _ = std::fs::remove_dir(&version);
    }
    let _ = std::fs::remove_dir(dir);
}

/// The ssh program: `PERCH_SSH` when set, for tests that wrap ssh with a
/// config of their own, else `ssh`.
pub fn ssh_program() -> std::ffi::OsString {
    std::env::var_os("PERCH_SSH")
        .filter(|program| !program.is_empty())
        .unwrap_or_else(|| "ssh".into())
}

/// Put `binary` on `host` as the `perchd` of its build, with a second ssh
/// that writes it from its stdin.
pub async fn upload(host: &str, binary: Binary, progress: &Progress) -> Result<(), Failed> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Binary {
        build,
        sum,
        bytes: binary,
    } = binary;
    let build = build.as_str();
    let mut child = tokio::process::Command::new(ssh_program())
        .args(super::remote::ssh_args(host, &upload_command(build, &sum)))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Failed::now(format!("Could not run ssh: {e}")))?;
    let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(Failed::now("ssh started with no pipes to it".to_string()));
    };
    phase(progress, "Connecting for upload");
    let send = async move {
        let total = binary.len() as u64;
        let mut completed = 0;
        let mut last_percent = None;
        progress.send_replace(Some(InstallProgress {
            phase: "Uploading perchd".into(),
            completed,
            total: Some(total),
        }));
        for chunk in binary.chunks(64 * 1024) {
            // A failed write is the script giving up early; its stderr says why.
            if stdin.write_all(chunk).await.is_err() {
                break;
            }
            completed += chunk.len() as u64;
            let update = InstallProgress {
                phase: "Uploading perchd".into(),
                completed,
                total: Some(total),
            };
            if update.percent() != last_percent {
                last_percent = update.percent();
                progress.send_replace(Some(update));
            }
        }
        if completed == total {
            phase(progress, "Checking what arrived");
        }
        let _ = stdin.shutdown().await;
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let done = async {
        let ((), read_out, read_err) = tokio::join!(
            send,
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        );
        let _ = (read_out, read_err);
        child.wait().await
    };
    let status = tokio::time::timeout(std::time::Duration::from_secs(600), done)
        .await
        .map_err(|_| Failed::now(format!("Sending perchd to {host} took over ten minutes.")))?
        .map_err(|e| Failed::now(format!("ssh to {host} failed: {e}")))?;
    let err = String::from_utf8_lossy(&err);
    for line in err.lines().filter(|line| !line.trim().is_empty()) {
        tracing::info!(host, "ssh (install): {}", line.trim());
    }
    let installed = String::from_utf8_lossy(&out)
        .lines()
        .any(|line| line.starts_with("perch-installed:"));
    if status.success() && installed {
        return Ok(());
    }
    let last = err
        .lines()
        .rev()
        .find(|line| line.starts_with("perch-"))
        .or_else(|| err.lines().rev().find(|line| !line.trim().is_empty()));
    Err(Failed::now(match last {
        Some(line) => upload_failure(line, host),
        None => format!("Couldn't put perchd on {host}: ssh exited with {status}."),
    }))
}

/// Whether `host` has the `perchd` of `build`. A host that can't be asked
/// has not, as far as sending one goes: the upload says why.
pub async fn has_build(host: &str, build: &str) -> bool {
    if !plain_word(build) {
        return false;
    }
    let run = tokio::process::Command::new(ssh_program())
        .args(super::remote::ssh_args(host, &has_command(build)))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(std::time::Duration::from_secs(30), run).await {
        Ok(Ok(output)) => {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.trim() == format!("perch-has: {build}"))
        }
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn uname_names_a_release_s_processor_or_says_what_to_do() {
        let (linux, mac) = (
            |arch| Platform::new(Os::Linux, arch),
            |arch| Platform::new(Os::MacOs, arch),
        );
        assert_eq!(platform("Linux", "x86_64"), Ok(linux(Arch::X86_64)));
        assert_eq!(platform("Linux", "aarch64"), Ok(linux(Arch::Arm64)));
        assert_eq!(platform("Linux", "arm64"), Ok(linux(Arch::Arm64)));
        assert_eq!(platform("Darwin", "arm64"), Ok(mac(Arch::Arm64)));
        assert_eq!(platform("Darwin", "x86_64"), Ok(mac(Arch::X86_64)));
        let why = platform("FreeBSD", "amd64").unwrap_err();
        assert!(why.contains("FreeBSD on amd64"), "{why}");
        assert!(why.contains("[remote] command"), "{why}");
        assert!(platform("Linux", "riscv64").is_err());
        assert_eq!(
            install_line("perch-install: Linux x86_64"),
            Some(Ok(linux(Arch::X86_64)))
        );
        assert_eq!(
            install_line("  perch-install: Darwin arm64 \n"),
            Some(Ok(mac(Arch::Arm64)))
        );
        assert!(matches!(
            install_line("perch-install: FreeBSD amd64"),
            Some(Err(_))
        ));
        assert_eq!(install_line("ssh: Could not resolve hostname halo"), None);
        assert_eq!(linux(Arch::Arm64).asset(), "perchd-linux-arm64.tar.gz");
        assert_eq!(mac(Arch::X86_64).asset(), "perchd-macos-x86_64.tar.gz");
        assert_eq!(mac(Arch::Arm64).name(), "macOS on arm64");
    }

    #[test]
    fn a_busy_engine_s_line_reads_back() {
        assert_eq!(
            busy_line("perch-busy: 0.9.0 2 1\n"),
            Some(Busy {
                version: "0.9.0".into(),
                runs: 2,
                clients: 1
            })
        );
        assert_eq!(busy_line("perch-busy: 0.9.0 two 1"), None);
        assert_eq!(busy_line("perch-install: Linux x86_64"), None);
        let builds = [(ANY.to_string(), "0123456789ab".to_string())];
        assert!(connect_command(&builds, Replace::IfIdle)
            .ends_with(" perch-connect --replace-if-idle any 0123456789ab'"));
        assert!(connect_command(&builds, Replace::Always)
            .ends_with(" perch-connect --replace any 0123456789ab'"));
        assert!(
            connect_command(&builds, Replace::No).ends_with(" perch-connect - any 0123456789ab'")
        );
        // With no build to name yet, the host can only say what it is.
        assert!(connect_command(&[], Replace::No).ends_with(" perch-connect -'"));
    }

    #[test]
    fn a_build_is_the_start_of_the_binary_s_hash() {
        // SHA-256 of "perchd" is 4d5c0a….
        let build = build_of(b"perchd");
        assert_eq!(build, hex(b"perchd")[..12]);
        assert_eq!(sum_of(b"perchd"), hex(b"perchd"));
        assert_ne!(build_of(b"perchd2"), build);
        assert!(plain_word(&build));
        assert!(plain_word("0.9.0-beta.1+git"));
        assert!(!plain_word("0.9;rm"));
        assert!(!plain_word(""));
        let keys: Vec<String> = Platform::ALL.into_iter().map(Platform::key).collect();
        assert_eq!(
            keys,
            ["linux-x86_64", "linux-arm64", "macos-x86_64", "macos-arm64"]
        );
    }

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn the_scripts_need_no_quote_any_login_shell_reads_its_own_way() {
        for script in [CONNECT, UPLOAD, HAS] {
            assert!(!script.contains(['\'', '\\', '`', '!']), "{script}");
        }
        let command = connect_command(
            &[("linux-x86_64".to_string(), "0123456789ab".to_string())],
            Replace::No,
        );
        assert!(command.starts_with("exec \"$SHELL\" -lc 'exec /bin/sh -c \""));
        assert!(command.ends_with("\" perch-connect - linux-x86_64 0123456789ab'"));
        // One pair of single quotes, around the whole of what the login
        // shell runs, and no backslash but before a `$` or a `"`.
        assert_eq!(command.matches('\'').count(), 2);
        let mut chars = command.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                assert!(matches!(chars.peek(), Some('$' | '"')), "{command}");
            }
        }
        let sum = sum_of(b"perchd");
        assert!(upload_command(&sum[..12], &sum)
            .ends_with(&format!("\" perch-upload {} {sum}'", &sum[..12])));
    }

    #[test]
    fn why_an_upload_failed_is_said() {
        let said = upload_failure(&format!("perch-sum: {}", sum_of(b"half")), "halo");
        assert!(said.contains("cut short or changed"), "{said}");
        let said = upload_failure("perch-sum: ", "halo");
        assert!(said.contains("no sha256sum, shasum or openssl"), "{said}");
        assert_eq!(
            upload_failure("perch-upload: could not make /x/bin", "halo"),
            "Couldn't put perchd on halo: could not make /x/bin."
        );
    }

    /// An ELF header for `machine`, and some bytes after it.
    fn elf(machine: u16) -> Vec<u8> {
        let mut binary = vec![0u8; 64];
        binary[..4].copy_from_slice(b"\x7fELF");
        binary[4] = 2;
        binary[18..20].copy_from_slice(&machine.to_le_bytes());
        binary.extend_from_slice(b"the rest of perchd");
        binary
    }

    /// A binary says what it runs on, and one read from a file is hashed
    /// as it is.
    #[test]
    fn a_binary_names_its_platform_and_its_build() {
        let binary = elf(62);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("perchd"), &binary).unwrap();
        let read = |name: &str| read_binary(&dir.path().join(name), "The perchd");
        let from_file = read("perchd").unwrap();
        assert_eq!(from_file.build, build_of(&binary));
        assert_eq!(from_file.sum, sum_of(&binary));
        assert_eq!(from_file.bytes, binary);
        assert!(read("nothing").is_err());
        std::fs::write(dir.path().join("script"), "#!/bin/sh\necho perchd\n").unwrap();
        let text = read("script").unwrap_err();
        assert!(
            text.contains("neither a Linux nor a macOS binary"),
            "{text}"
        );
        let (linux, mac) = (
            |arch| Platform::new(Os::Linux, arch),
            |arch| Platform::new(Os::MacOs, arch),
        );
        assert_eq!(built_for(&elf(183)), Ok(vec![linux(Arch::Arm64)]));
        assert!(built_for(&elf(40)).is_err());
        // A Mach-O names its processor after its magic; a universal one
        // lists each it holds.
        let mut thin = b"\xcf\xfa\xed\xfe".to_vec();
        thin.extend(0x0100_000cu32.to_le_bytes());
        assert_eq!(built_for(&thin), Ok(vec![mac(Arch::Arm64)]));
        let mut fat = b"\xca\xfe\xba\xbe".to_vec();
        fat.extend(2u32.to_be_bytes());
        for cpu in [0x0100_0007u32, 0x0100_000c] {
            fat.extend(cpu.to_be_bytes());
            fat.extend([0u8; 16]);
        }
        assert_eq!(
            built_for(&fat),
            Ok(vec![mac(Arch::X86_64), mac(Arch::Arm64)])
        );
        assert!(built_for(b"\xcf\xfa\xed\xfe\x07\x00\x00\x00").is_err());
    }

    /// An engine the app carries unpacks to the binary its build hashed,
    /// and one that does not, or is another platform's, is not sent.
    #[test]
    fn an_engine_the_app_carries_is_checked_as_it_is_unpacked() {
        let (x86, arm) = (
            Platform::new(Os::Linux, Arch::X86_64),
            Platform::new(Os::Linux, Arch::Arm64),
        );
        let carried = Carried::pack(x86, &elf(62));
        assert_eq!(carried.build(), build_of(&elf(62)));
        assert_eq!(
            format!("{carried:?}"),
            format!("linux-x86_64 {}", build_of(&elf(62)))
        );
        let binary = carried.binary().unwrap();
        assert_eq!(binary.bytes, elf(62));
        assert_eq!(binary.build, carried.build());

        // As the app's build hands them over: a platform's key, the
        // hash, and the bytes.
        let packed: &'static [u8] = Vec::leak(carried.packed.to_vec());
        let embedded = Carried::embedded("linux-x86_64", &carried.sum.to_uppercase(), packed);
        assert_eq!(embedded, Some(carried.clone()));
        assert_eq!(
            Carried::embedded("plan9-x86_64", &carried.sum, packed),
            None
        );
        assert_eq!(
            Carried::embedded("linux-x86_64", "0123456789ab", packed),
            None
        );

        // Another hash than the bytes have, another platform's binary,
        // and bytes that are no gzip at all.
        let wrong_sum = Carried {
            sum: sum_of(b"another"),
            ..carried.clone()
        };
        let why = wrong_sum.binary().unwrap_err();
        assert!(why.lasting && why.why.contains("not the binary it was built with"));
        let why = Carried::pack(arm, &elf(62)).binary().unwrap_err();
        assert!(why.why.contains("Linux on arm64"), "{}", why.why);
        assert!(why.why.contains("another platform"), "{}", why.why);
        let broken = Carried {
            packed: b"not gzip".to_vec().into(),
            ..carried.clone()
        };
        assert!(broken.binary().unwrap_err().why.contains("does not unpack"));

        let source = Source {
            carried: vec![carried.clone()],
            workspace: None,
        };
        assert_eq!(source.carried_for(x86), Some(&carried));
        assert_eq!(source.carried_for(arm), None);
        let none = none_carried(arm);
        assert!(
            none.contains("carries no perchd for Linux on arm64"),
            "{none}"
        );
        assert!(none.contains("[remote] command"), "{none}");
    }

    /// What an older app downloaded goes, and nothing else does.
    #[test]
    fn the_downloads_an_older_app_kept_are_cleared() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("perchd");
        let write = |path: &str| {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        };
        for path in [
            "0.8.0/SHA256SUMS",
            "0.8.0/perchd-linux-x86_64.tar.gz",
            "0.9.0/SHA256SUMS",
            "0.9.0/perchd-macos-arm64.tar.gz",
            "0.9.0/perchd-linux-arm64.tar.part",
            "0.9.0/notes.txt",
        ] {
            write(path);
        }
        clear_old_downloads(&dir);
        assert!(!dir.join("0.8.0").exists());
        let left: Vec<_> = std::fs::read_dir(dir.join("0.9.0"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["notes.txt"]);

        // With nothing else in it, the folder itself goes; with none, nothing.
        std::fs::remove_file(dir.join("0.9.0/notes.txt")).unwrap();
        clear_old_downloads(&dir);
        assert!(!dir.exists());
        clear_old_downloads(&dir);
    }

    /// The shells a login shell may be that are here: bash and zsh on
    /// macOS, fish when installed.
    pub(crate) fn shells() -> Vec<String> {
        [
            "/bin/bash",
            "/bin/zsh",
            "/usr/bin/zsh",
            "/bin/sh",
            "/bin/dash",
            "/usr/bin/dash",
        ]
        .into_iter()
        .map(String::from)
        .chain(
            std::env::var_os("PATH")
                .iter()
                .flat_map(std::env::split_paths)
                .map(|dir| dir.join("fish"))
                .find(|fish| fish.is_file())
                .map(|fish| fish.display().to_string()),
        )
        .filter(|shell| Path::new(shell).is_file())
        .collect()
    }

    /// The upload script, run as ssh would have each login shell run it,
    /// with a stand-in for perchd on stdin: one that arrives whole is put
    /// in place under its build, owner-only, and one that arrives as
    /// anything else is not, and leaves nothing behind.
    #[test]
    fn the_upload_script_keeps_only_what_was_sent() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        for shell in shells() {
            let home = tempfile::tempdir().unwrap();
            let upload = |sent: &str, binary: &str| {
                let sum = sum_of(binary.as_bytes());
                let mut child = std::process::Command::new(&shell)
                    .arg("-c")
                    .arg(upload_command(&sum[..BUILD_DIGITS], &sum))
                    .env_clear()
                    .env("HOME", home.path())
                    .env("SHELL", &shell)
                    .env("PATH", "/usr/bin:/bin")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(sent.as_bytes())
                    .unwrap();
                child.wait_with_output().unwrap()
            };
            let bin = home.path().join(".perch/bin");
            let good = "#!/bin/sh\necho perchd\n";
            let done = upload(good, good);
            assert!(
                done.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&done.stderr)
            );
            let name = format!("perchd-{}", build_of(good.as_bytes()));
            let installed = bin.join(&name);
            assert_eq!(std::fs::read_to_string(&installed).unwrap(), good);
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&installed), 0o700, "{shell}");
            assert_eq!(mode(&bin), 0o700, "{shell}");
            assert!(String::from_utf8_lossy(&done.stdout).contains("perch-installed:"));

            // Sent again, as by another app of the same build at the same
            // time: the file that is there stays the file, for a bridge
            // already running from it.
            let inode = |path: &Path| {
                use std::os::unix::fs::MetadataExt;
                std::fs::metadata(path).unwrap().ino()
            };
            let before = inode(&installed);
            assert!(upload(good, good).status.success(), "{shell}");
            assert_eq!(inode(&installed), before, "{shell}");
            // One left there that can't be run is replaced.
            std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert!(upload(good, good).status.success(), "{shell}");
            assert_eq!(mode(&installed), 0o700, "{shell}");

            // Cut short on the way: half of another binary.
            let other = "#!/bin/sh\necho another perchd\n";
            let refused = upload(&other[..other.len() / 2], other);
            assert!(!refused.status.success(), "{shell}");
            let said = String::from_utf8_lossy(&refused.stderr);
            let last = said.lines().last().unwrap_or_default();
            assert_eq!(
                last,
                format!(
                    "perch-sum: {}",
                    sum_of(&other.as_bytes()[..other.len() / 2])
                ),
                "{shell}"
            );
            assert!(upload_failure(last, "halo").contains("cut short or changed"));
            let left: Vec<_> = std::fs::read_dir(&bin)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(left, [name], "{shell}");
        }
    }

    /// A dev build's `perchd` is the toolchain's own for this machine and
    /// for another processor of its system, and zig's for another system,
    /// against musl for Linux; what cargo says is missing is what the
    /// reader is told to install.
    #[test]
    fn perchd_is_built_from_source_for_what_the_host_is() {
        let (mac, linux) = (
            |arch| Platform::new(Os::MacOs, arch),
            |arch| Platform::new(Os::Linux, arch),
        );
        let here = Some(mac(Arch::Arm64));
        let build = |subcommand, triple| DevBuild { subcommand, triple };
        assert_eq!(dev_build_for(mac(Arch::Arm64), here), build("build", None));
        assert_eq!(
            dev_build_for(mac(Arch::X86_64), here),
            build("build", Some("x86_64-apple-darwin"))
        );
        let cross = dev_build_for(linux(Arch::X86_64), here);
        assert_eq!(cross, build("zigbuild", Some("x86_64-unknown-linux-musl")));
        // From Linux, a Linux host's is still static, its own processor's
        // included, and through zig for the C in it.
        for from in [linux(Arch::X86_64), linux(Arch::Arm64)] {
            assert_eq!(
                dev_build_for(linux(Arch::X86_64), Some(from)),
                build("zigbuild", Some("x86_64-unknown-linux-musl"))
            );
        }
        assert_eq!(
            dev_build_for(mac(Arch::Arm64), Some(linux(Arch::Arm64))),
            build("zigbuild", Some("aarch64-apple-darwin"))
        );
        assert_eq!(dev_build_for(mac(Arch::Arm64), None).subcommand, "zigbuild");

        // Cargo says where it put the binary.
        let messages = concat!(
            r#"{"reason":"compiler-artifact","target":{"name":"perch-engine"},"executable":null}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"name":"perchd"},"executable":"/t/x86_64-unknown-linux-musl/debug/perchd"}"#,
            "\nnot json\n",
            r#"{"reason":"build-finished","success":true}"#,
        );
        assert_eq!(
            built_perchd(messages),
            Some(PathBuf::from("/t/x86_64-unknown-linux-musl/debug/perchd"))
        );
        assert_eq!(built_perchd(r#"{"reason":"build-finished"}"#), None);

        let host = linux(Arch::X86_64);
        let no_zigbuild = dev_build_failure(cross, host, "error: no such command: `zigbuild`\n");
        assert!(
            no_zigbuild.contains("cargo install cargo-zigbuild"),
            "{no_zigbuild}"
        );
        let no_target = dev_build_failure(
            cross,
            host,
            "error[E0463]: can't find crate for `core`\n  = note: the `x86_64-unknown-linux-musl` target may not be installed\n",
        );
        assert!(
            no_target.ends_with("rustup target add x86_64-unknown-linux-musl"),
            "{no_target}"
        );
        let broken = dev_build_failure(
            cross,
            host,
            "   Compiling perchd\nerror[E0425]: cannot find value `x`\nerror: could not compile `perchd`\n",
        );
        assert_eq!(
            broken,
            "Couldn't build perchd for Linux on x86_64: error: could not compile `perchd`"
        );
    }

    /// The script that says whether a build is installed, in each login
    /// shell: it is only once the file is there to run.
    #[test]
    fn a_host_says_whether_it_has_a_build() {
        use std::os::unix::fs::PermissionsExt;
        for shell in shells() {
            let home = tempfile::tempdir().unwrap();
            let has = |build: &str| {
                std::process::Command::new(&shell)
                    .arg("-c")
                    .arg(has_command(build))
                    .env_clear()
                    .env("HOME", home.path())
                    .env("SHELL", &shell)
                    .env("PATH", "/usr/bin:/bin")
                    .output()
                    .unwrap()
            };
            assert!(!has("0123456789ab").status.success(), "{shell}");
            let bin = home.path().join(".perch/bin");
            std::fs::create_dir_all(&bin).unwrap();
            let file = bin.join("perchd-0123456789ab");
            std::fs::write(&file, "").unwrap();
            assert!(!has("0123456789ab").status.success(), "{shell}");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
            let there = has("0123456789ab");
            assert!(there.status.success(), "{shell}");
            assert_eq!(
                String::from_utf8_lossy(&there.stdout).trim(),
                "perch-has: 0123456789ab"
            );
            assert!(!has("ba9876543210").status.success(), "{shell}");
        }
    }

    /// An upload whose script is told to stop while the binary is still
    /// coming, as when the ssh session ends under it, leaves no temporary
    /// file behind, in any login shell.
    #[test]
    fn an_upload_cut_short_leaves_nothing_behind() {
        use std::io::Write;
        use std::time::{Duration, Instant};
        for shell in shells() {
            let home = tempfile::tempdir().unwrap();
            let sum = sum_of(b"the whole of perchd");
            let mut child = std::process::Command::new(&shell)
                .arg("-c")
                .arg(upload_command(&sum[..BUILD_DIGITS], &sum))
                .env_clear()
                .env("HOME", home.path())
                .env("SHELL", &shell)
                .env("PATH", "/usr/bin:/bin")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            // Some of it, and the way in left open: the script is still
            // reading when it is told.
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(b"the whole").unwrap();
            stdin.flush().unwrap();
            let bin = home.path().join(".perch/bin");
            let deadline = Instant::now() + Duration::from_secs(10);
            let written = loop {
                let ready = std::fs::read_dir(&bin)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .any(|entry| entry.file_name().to_string_lossy().starts_with(".perchd-"));
                if ready || Instant::now() > deadline {
                    break ready;
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            assert!(
                written,
                "{shell}: the script never wrote its temporary file"
            );
            let told = std::process::Command::new("kill")
                .args(["-HUP", &child.id().to_string()])
                .status()
                .unwrap();
            assert!(told.success(), "{shell}");
            // The session's end closes the way in, too: the shell takes
            // the signal once what it was waiting on is done.
            drop(stdin);
            assert!(!child.wait().unwrap().success(), "{shell}");
            let left: Vec<_> = std::fs::read_dir(&bin)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert!(left.is_empty(), "{shell}: left {left:?}");
        }
    }
}
