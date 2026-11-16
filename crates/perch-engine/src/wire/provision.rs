//! Putting `perchd` on a host. With no `[remote] command`, the app runs
//! the `perchd` it installed there itself, by a build it names:
//! `${PERCH_HOME:-$HOME/.perch}/bin/perchd-<build>`. When that is not
//! there, the host says so on stderr and the link installs it, with a
//! second ssh, and dials again.
//!
//! - `<build>` is the app's version for a release's `perchd`, and
//!   `<version>-<first 12 hex of the binary's SHA-256>` for one named by
//!   `[remote] perchd`, so a rebuilt dev binary is installed again. The
//!   binary is hashed, not the file: a tarball and the binary in it name
//!   the same build.
//! - A release's tarball is downloaded with no token, checked against the
//!   release's `SHA256SUMS`, and kept in `~/.perch/perchd/<version>/`. The
//!   binary is taken out of it here, on the Mac.
//! - What ssh runs on the host goes through the reader's login shell,
//!   which may be bash, zsh or fish, so the engine gets the `PATH` their
//!   profile sets. The logic is a POSIX `sh` script, and the quoting around
//!   it holds no single quotes and no backslash a shell would read as one.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{PROTOCOL, VERSION};

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

/// The largest `perchd` (or tarball of one) taken: a release's is around
/// 20 MB.
const MAX_BINARY: u64 = 256 << 20;

/// The largest `SHA256SUMS` taken.
const MAX_SUMS: u64 = 1 << 20;

/// Where the `perchd` installed comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The release of the app's own version, downloaded.
    Release,
    /// `[remote] perchd`: a Linux binary, or a release tarball of one, on
    /// this machine.
    File(PathBuf),
    /// `[remote] perchd_on_host`: a Linux binary on the host, as one built
    /// there from the source the app was built from. Nothing is uploaded:
    /// the host copies it into place itself.
    HostFile(String),
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

    /// The release's tarball for it.
    pub fn asset(self) -> String {
        format!("perchd-linux-{}.tar.gz", self.name())
    }

    /// Its number in an ELF header's `e_machine`.
    fn elf_machine(self) -> u16 {
        match self {
            Arch::X86_64 => 62,
            Arch::Arm64 => 183,
        }
    }
}

/// The processor `uname -s` and `uname -m` name, or why Perch has no
/// `perchd` for it.
pub fn platform(os: &str, machine: &str) -> Result<Arch, String> {
    match (os, machine) {
        ("Linux", "x86_64" | "amd64") => Ok(Arch::X86_64),
        ("Linux", "aarch64" | "arm64") => Ok(Arch::Arm64),
        _ => Err(format!(
            "The host is {os} on {machine}, and Perch has perchd only for Linux on x86_64 and arm64. Install perchd there by hand and set [remote] command to run it."
        )),
    }
}

/// What a line the host said asks for: None when it is not the line
/// saying `perchd` is missing, else the processor to install for, or why
/// there is none.
pub fn install_line(line: &str) -> Option<Result<Arch, String>> {
    let rest = line.trim().strip_prefix(INSTALL_SAID)?;
    let mut words = rest.split_whitespace();
    let (os, machine) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    Some(platform(os, machine))
}

/// The build of a release's `perchd`: the app's version.
pub fn release_build() -> String {
    VERSION.to_string()
}

/// The build of `binary`, a `perchd` named by `[remote] perchd`.
pub fn file_build(binary: &[u8]) -> String {
    let digest = Sha256::digest(binary);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{VERSION}-{}", &hex[..12])
}

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

/// `[remote] perchd_on_host` as the staging script takes it: from the
/// host's home when it starts with `~/` or names no root, else as it is.
/// Err when it holds a character Perch won't put on a command line.
pub fn host_path(path: &str) -> Result<String, String> {
    let path = path.trim();
    let from_home = path.strip_prefix("~/").unwrap_or(path);
    if plain_path(from_home) && !from_home.starts_with('-') {
        Ok(from_home.to_string())
    } else {
        Err(format!(
            "[remote] perchd_on_host, {path}, can hold only letters, digits and . _ + - /, after a ~/ if it is under the home directory."
        ))
    }
}

/// The start of what the staging script says on stdout when the `perchd`
/// on the host is in place, before its build.
const STAGED_SAID: &str = "perch-staged:";

/// The staging script, run by `sh` with the path of a `perchd` on the
/// host, the version and the protocol as `$1`, `$2` and `$3`. It names the
/// file's build as the app would, `<version>-<first 12 hex of its
/// SHA-256>`, and when no `perchd` of that build is installed copies the
/// file into place the way an upload is: a temporary file, owner-only,
/// asked its version, moved only when that is the version and protocol
/// expected. A file that changed while it was copied, as under a build
/// still running, is not installed.
const STAGE: &str = r#"d="${PERCH_HOME:-$HOME/.perch}/bin"
case "$1" in
  /*) f="$1" ;;
  *) f="$HOME/$1" ;;
esac
if [ -f "$f" ] && [ -x "$f" ]; then
  :
else
  echo "perch-stage: there is no perchd at $f" >&2
  exit 1
fi
sum() {
  { sha256sum "$1" 2>/dev/null || shasum -a 256 "$1"; } | cut -c1-12
}
h=$(sum "$f")
case "$h" in
  ????????????) ;;
  *)
    echo "perch-stage: could not take the SHA-256 of $f" >&2
    exit 1
    ;;
esac
b="$2-$h"
if [ -x "$d/perchd-$b" ]; then
  echo "perch-staged: $b"
  exit 0
fi
umask 077
mkdir -p "$d" && chmod 700 "$d" || {
  echo "perch-stage: could not make $d" >&2
  exit 1
}
t="$d/.perchd-$b.$$"
gone() {
  rm -f "$t"
}
trap gone EXIT
trap "exit 1" HUP INT PIPE TERM
cp "$f" "$t" && chmod 700 "$t" || {
  echo "perch-stage: could not copy $f to $t" >&2
  exit 1
}
if [ "$(sum "$t")" = "$h" ]; then
  :
else
  echo "perch-stage: $f changed while it was copied" >&2
  exit 1
fi
v=$("$t" --version 2>&1)
case "$v" in
  "perchd $2 (protocol $3)") ;;
  *)
    echo "perch-version: $v" >&2
    exit 1
    ;;
esac
mv -f "$t" "$d/perchd-$b" || {
  echo "perch-stage: could not move it to $d/perchd-$b" >&2
  exit 1
}
echo "perch-staged: $b""#;

/// The connection script, run by `sh` with the build as `$1` and the
/// bridge's flags after it: the bridge of that build if it is there, else
/// a line saying what the host is, for the app to install from.
const CONNECT: &str = r#"d="${PERCH_HOME:-$HOME/.perch}/bin"
b="$1"
shift
if [ -x "$d/perchd-$b" ]; then
  exec "$d/perchd-$b" bridge "$@"
fi
echo "perch-install: $(uname -s) $(uname -m)" >&2
exit 97"#;

/// The upload script, run by `sh` with the build, the version and the
/// protocol as `$1`, `$2` and `$3`, and the binary on stdin. It writes a
/// temporary file beside the others, owner-only, asks it its version, and
/// moves it into place only when that is the version and protocol
/// expected. The temporary file goes whatever fails, the ssh session
/// ending under it included: a write to its closed stderr is a SIGPIPE,
/// which the trap turns into an exit, so the file goes with it. One left
/// by a script killed outright is the bridge's to clear.
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
v=$("$t" --version 2>&1)
case "$v" in
  "perchd $2 (protocol $3)") ;;
  *)
    echo "perch-version: $v" >&2
    exit 1
    ;;
esac
mv -f "$t" "$d/perchd-$1" || {
  echo "perch-upload: could not move it to $d/perchd-$1" >&2
  exit 1
}
echo "perch-installed: $d/perchd-$1""#;

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

/// What ssh runs on the host to reach the engine through the `perchd` of
/// `build`, replacing one of another build as `replace` says, or to say
/// it is missing.
pub fn connect_command(build: &str, replace: Replace) -> String {
    let mut args = vec![build];
    args.extend(replace.flag());
    in_login_sh("perch-connect", CONNECT, &args)
}

/// What ssh runs on the host to install the `perchd` of `build` from its
/// stdin.
pub fn upload_command(build: &str) -> String {
    in_login_sh(
        "perch-upload",
        UPLOAD,
        &[build, VERSION, &PROTOCOL.to_string()],
    )
}

/// What ssh runs on the host to put the `perchd` at `path` there in place,
/// `path` as [`host_path`] gives it.
pub fn stage_command(path: &str) -> String {
    in_login_sh(
        "perch-stage",
        STAGE,
        &[path, VERSION, &PROTOCOL.to_string()],
    )
}

/// What a failed staging's last line says, put for the reader.
pub fn stage_failure(line: &str, host: &str) -> String {
    if let Some(said) = line.trim().strip_prefix("perch-version:") {
        let said = said.trim();
        return match parse_version(said) {
            Some((version, protocol)) => format!(
                "[remote] perchd_on_host on {host} is perchd {version} (protocol {protocol}), and this app is {VERSION} (protocol {PROTOCOL}). Build both from the same source."
            ),
            None if said.is_empty() => format!(
                "[remote] perchd_on_host on {host} printed nothing when asked its version."
            ),
            None => format!("[remote] perchd_on_host on {host} would not run: {said}"),
        };
    }
    match line.trim().strip_prefix("perch-stage:") {
        Some(said) => format!("[remote] perchd_on_host on {host}: {}.", said.trim()),
        None => format!("Couldn't ready perchd on {host}: {}", line.trim()),
    }
}

/// The version and protocol in what `perchd --version` prints:
/// `perchd 0.9.0 (protocol 4)`.
pub fn parse_version(line: &str) -> Option<(String, u32)> {
    let rest = line.trim().strip_prefix("perchd ")?;
    let (version, rest) = rest.split_once(' ')?;
    let protocol = rest.strip_prefix("(protocol ")?.strip_suffix(')')?;
    Some((version.to_string(), protocol.parse().ok()?))
}

/// What a failed upload's last line says, put for the reader.
pub fn upload_failure(line: &str, host: &str) -> String {
    if let Some(said) = line.trim().strip_prefix("perch-version:") {
        let said = said.trim();
        return match parse_version(said) {
            Some((version, protocol)) => format!(
                "The perchd uploaded to {host} is {version} (protocol {protocol}), and this app is {VERSION} (protocol {PROTOCOL}), so it was not installed."
            ),
            None if said.is_empty() => format!(
                "The perchd uploaded to {host} printed nothing when asked its version, so it was not installed."
            ),
            None => format!("The perchd uploaded to {host} would not run there: {said}"),
        };
    }
    match line.trim().strip_prefix("perch-upload:") {
        Some(said) => format!("Couldn't install perchd on {host}: {}.", said.trim()),
        None => format!("Couldn't install perchd on {host}: {}", line.trim()),
    }
}

/// The checksum `SHA256SUMS` gives for `name`, as `sha256sum` writes it:
/// `<hex>  <name>`, or `<hex> *<name>` for one read as binary.
fn listed_sum<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (hex, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start();
        (file.strip_prefix('*').unwrap_or(file) == name).then_some(hex)
    })
}

/// Whether `bytes` are the `name` that `sums` lists.
pub fn check_sum(sums: &str, name: &str, bytes: &[u8]) -> Result<(), String> {
    let Some(listed) = listed_sum(sums, name) else {
        return Err(format!("SHA256SUMS does not list {name}."));
    };
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if hex.eq_ignore_ascii_case(listed) {
        Ok(())
    } else {
        Err(format!(
            "{name} does not match its SHA-256 in SHA256SUMS: it came down damaged or was changed."
        ))
    }
}

/// The file named `perchd` in a gzipped tarball, at its top or in a
/// directory. Only plain files count, so neither a link nor macOS's
/// `._perchd` is taken for it.
pub fn extract_perchd(tarball: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(tarball)
        .take(MAX_BINARY * 2)
        .read_to_end(&mut tar)
        .map_err(|e| format!("it is not a gzipped tarball: {e}"))?;
    let mut at = 0;
    // A GNU long name, for the entry after it.
    let mut long_name: Option<String> = None;
    while at + 512 <= tar.len() {
        let header = &tar[at..at + 512];
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let size = tar_size(&header[124..136])
            .ok_or_else(|| "a tarball entry has a size that does not read".to_string())?;
        let data = at + 512;
        let end = data
            .checked_add(usize::try_from(size).map_err(|_| "a tarball entry too large")?)
            .filter(|&end| end <= tar.len())
            .ok_or_else(|| "the tarball ends inside an entry".to_string())?;
        let kind = header[156];
        let name = match long_name.take() {
            Some(name) => name,
            None => {
                let field = |range: std::ops::Range<usize>| {
                    let bytes = &header[range];
                    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                    String::from_utf8_lossy(&bytes[..end]).into_owned()
                };
                let (name, prefix) = (field(0..100), field(345..500));
                if &header[257..262] == b"ustar" && !prefix.is_empty() {
                    format!("{prefix}/{name}")
                } else {
                    name
                }
            }
        };
        match kind {
            b'L' => {
                let bytes = &tar[data..end];
                let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                long_name = Some(String::from_utf8_lossy(&bytes[..len]).into_owned());
            }
            b'0' | 0 if Path::new(&name).file_name() == Some("perchd".as_ref()) => {
                return Ok(tar[data..end].to_vec());
            }
            _ => {}
        }
        at = data + (end - data).div_ceil(512) * 512;
    }
    Err("the tarball has no perchd in it".to_string())
}

/// A tar header's size: octal digits, or GNU's base-256 for large ones.
fn tar_size(field: &[u8]) -> Option<u64> {
    if field[0] & 0x80 != 0 {
        return field[1..]
            .iter()
            .try_fold(0u64, |n, &b| n.checked_mul(256)?.checked_add(u64::from(b)));
    }
    let text = std::str::from_utf8(field).ok()?;
    let digits = text.trim_matches(|c: char| c == '\0' || c == ' ');
    if digits.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(digits, 8).ok()
}

/// The processor a Linux binary is for, or why it is not one.
pub fn elf_arch(binary: &[u8]) -> Result<Arch, String> {
    if binary.len() < 20 || &binary[..4] != b"\x7fELF" {
        return Err("it is not a Linux binary".to_string());
    }
    let machine = u16::from_le_bytes([binary[18], binary[19]]);
    [Arch::X86_64, Arch::Arm64]
        .into_iter()
        .find(|arch| arch.elf_machine() == machine)
        .ok_or_else(|| format!("it is a binary for neither x86_64 nor arm64 (machine {machine})"))
}

/// The `perchd` in what `[remote] perchd` names: the file itself, or the
/// one in it when it is a gzipped tarball.
pub fn binary_in(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    let binary = if bytes.starts_with(&[0x1f, 0x8b]) {
        extract_perchd(&bytes)?
    } else {
        bytes
    };
    elf_arch(&binary)?;
    Ok(binary)
}

/// A `perchd` to install, and its build.
#[derive(Debug, Clone)]
pub struct Binary {
    pub build: String,
    pub bytes: Vec<u8>,
}

/// The `perchd` `[remote] perchd` names, read now: a file rebuilt while the
/// app runs is installed on the next connect.
pub fn read_file(path: &Path) -> Result<Binary, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Couldn't read [remote] perchd, {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BINARY {
        return Err(format!(
            "[remote] perchd, {}, is larger than any perchd.",
            path.display()
        ));
    }
    let bytes = binary_in(bytes).map_err(|why| {
        format!(
            "[remote] perchd, {}, can't be installed: {why}.",
            path.display()
        )
    })?;
    Ok(Binary {
        build: file_build(&bytes),
        bytes,
    })
}

/// Why an install did not happen, and whether trying again could help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub why: String,
    /// Trying again cannot help until the reader changes something: a
    /// release with no `perchd`, a host Perch has none for.
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

/// Where the release's files are kept on this machine.
fn cache_dir() -> PathBuf {
    perch_github::config::perch_home()
        .join("perchd")
        .join(VERSION)
}

/// Write `bytes` to `path` whole or not at all.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = path.with_extension("part");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)
}

/// The release's `perchd` for `arch`, from the cache when what is there
/// checks out, else downloaded and kept.
pub async fn release_binary(arch: Arch) -> Result<Vec<u8>, Failed> {
    let (dir, asset) = (cache_dir(), arch.asset());
    let (sums_path, tarball_path) = (dir.join("SHA256SUMS"), dir.join(&asset));
    let cached = {
        let (sums_path, tarball_path, asset) =
            (sums_path.clone(), tarball_path.clone(), asset.clone());
        tokio::task::spawn_blocking(move || {
            let sums = std::fs::read_to_string(&sums_path).ok()?;
            let tarball = std::fs::read(&tarball_path).ok()?;
            check_sum(&sums, &asset, &tarball).ok()?;
            Some(tarball)
        })
        .await
        .ok()
        .flatten()
    };
    let tarball = match cached {
        Some(tarball) => tarball,
        None => {
            let missing = |name: &str| {
                Failed::lasting(format!(
                    "Perch's v{VERSION} release has no {name} to download. Set [remote] perchd to a perchd built for Linux on {}, or [remote] command to run one installed by hand.",
                    arch.name()
                ))
            };
            let fetch = |name: String| async move {
                let url = perch_github::releases::perch_download_url(VERSION, &name);
                let limit = if name == "SHA256SUMS" {
                    MAX_SUMS
                } else {
                    MAX_BINARY
                };
                perch_github::releases::download(&url, limit)
                    .await
                    .map_err(|error| match error {
                        perch_github::releases::DownloadError::Missing => missing(&name),
                        perch_github::releases::DownloadError::Failed(why) => Failed::now(why),
                    })
            };
            let sums = fetch("SHA256SUMS".to_string()).await?;
            let sums = String::from_utf8_lossy(&sums).into_owned();
            let tarball = fetch(asset.clone()).await?;
            check_sum(&sums, &asset, &tarball).map_err(Failed::now)?;
            let kept = std::fs::create_dir_all(&dir)
                .and_then(|()| write_whole(&sums_path, sums.as_bytes()))
                .and_then(|()| write_whole(&tarball_path, &tarball));
            if let Err(error) = kept {
                tracing::warn!(dir = %dir.display(), %error, "could not keep the perchd download");
            }
            tarball
        }
    };
    tokio::task::spawn_blocking(move || extract_perchd(&tarball))
        .await
        .map_err(|e| Failed::now(e.to_string()))?
        .map_err(|why| Failed::now(format!("The release's {asset} can't be used: {why}.")))
}

/// The ssh program: `PERCH_SSH` when set, for tests that wrap ssh with a
/// config of their own, else `ssh`.
pub fn ssh_program() -> std::ffi::OsString {
    std::env::var_os("PERCH_SSH")
        .filter(|program| !program.is_empty())
        .unwrap_or_else(|| "ssh".into())
}

/// Install `binary` as the `perchd` of `build` on `host`, with a second ssh
/// that writes it from its stdin.
pub async fn upload(host: &str, build: &str, binary: Vec<u8>) -> Result<(), Failed> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    if !plain_word(build) {
        return Err(Failed::lasting(format!(
            "perchd's build, {build}, has a character Perch won't put on a command line."
        )));
    }
    let mut child = tokio::process::Command::new(ssh_program())
        .args(super::remote::ssh_args(host, &upload_command(build)))
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
    let send = async move {
        // A failed write is the script giving up early; its stderr says why.
        let _ = stdin.write_all(&binary).await;
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
        .map_err(|_| {
            Failed::now(format!(
                "Installing perchd on {host} took over ten minutes."
            ))
        })?
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
        None => format!("Couldn't install perchd on {host}: ssh exited with {status}."),
    }))
}

/// Put the `perchd` at `path` on `host` in place there, if its build is
/// not already, and give its build: an ssh of its own before the one that
/// connects, since the build is in the name the connection runs.
pub async fn stage(host: &str, path: &str) -> Result<String, String> {
    let path = host_path(path)?;
    let run = tokio::process::Command::new(ssh_program())
        .args(super::remote::ssh_args(host, &stage_command(&path)))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(std::time::Duration::from_secs(120), run)
        .await
        .map_err(|_| format!("Readying perchd on {host} took over two minutes."))?
        .map_err(|e| format!("Could not run ssh: {e}"))?;
    let err = String::from_utf8_lossy(&output.stderr);
    for line in err.lines().filter(|line| !line.trim().is_empty()) {
        tracing::info!(host, "ssh (stage): {}", line.trim());
    }
    let out = String::from_utf8_lossy(&output.stdout);
    let staged = out
        .lines()
        .find_map(|line| line.trim().strip_prefix(STAGED_SAID))
        .map(|build| build.trim().to_string());
    match staged {
        Some(build) if output.status.success() && plain_word(&build) => Ok(build),
        _ => {
            let last = err
                .lines()
                .rev()
                .find(|line| line.starts_with("perch-"))
                .or_else(|| err.lines().rev().find(|line| !line.trim().is_empty()));
            Err(match last {
                Some(line) => stage_failure(line, host),
                None => format!(
                    "Couldn't ready perchd on {host}: ssh exited with {}.",
                    output.status
                ),
            })
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn uname_names_a_release_s_processor_or_says_what_to_do() {
        assert_eq!(platform("Linux", "x86_64"), Ok(Arch::X86_64));
        assert_eq!(platform("Linux", "aarch64"), Ok(Arch::Arm64));
        assert_eq!(platform("Linux", "arm64"), Ok(Arch::Arm64));
        let why = platform("Darwin", "arm64").unwrap_err();
        assert!(why.contains("Darwin on arm64"), "{why}");
        assert!(why.contains("[remote] command"), "{why}");
        assert!(platform("Linux", "riscv64").is_err());
        assert_eq!(
            install_line("perch-install: Linux x86_64"),
            Some(Ok(Arch::X86_64))
        );
        assert_eq!(
            install_line("  perch-install: Linux aarch64 \n"),
            Some(Ok(Arch::Arm64))
        );
        assert!(matches!(
            install_line("perch-install: FreeBSD amd64"),
            Some(Err(_))
        ));
        assert_eq!(install_line("ssh: Could not resolve hostname halo"), None);
        assert_eq!(Arch::Arm64.asset(), "perchd-linux-arm64.tar.gz");
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
        assert!(connect_command("0.9.0", Replace::IfIdle).contains(" 0.9.0 --replace-if-idle'"));
        assert!(connect_command("0.9.0", Replace::Always).ends_with(" 0.9.0 --replace'"));
        assert!(connect_command("0.9.0", Replace::No).ends_with(" 0.9.0'"));
    }

    #[test]
    fn a_build_is_the_version_or_the_version_and_the_binary_s_hash() {
        assert_eq!(release_build(), VERSION);
        // SHA-256 of "perchd" is 4d5c0a….
        let build = file_build(b"perchd");
        assert_eq!(build, format!("{VERSION}-{}", &hex(b"perchd")[..12]));
        assert_eq!(build.len(), VERSION.len() + 13);
        assert_ne!(file_build(b"perchd2"), build);
        assert!(plain_word(&build));
        assert!(plain_word("0.9.0-beta.1+git"));
        assert!(!plain_word("0.9;rm"));
        assert!(!plain_word(""));
    }

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn the_scripts_need_no_quote_any_login_shell_reads_its_own_way() {
        for script in [CONNECT, UPLOAD, STAGE] {
            assert!(!script.contains(['\'', '\\', '`', '!']), "{script}");
        }
        let command = connect_command("0.9.0", Replace::No);
        assert!(command.starts_with("exec \"$SHELL\" -lc 'exec /bin/sh -c \""));
        assert!(command.ends_with("\" perch-connect 0.9.0'"));
        // One pair of single quotes, around the whole of what the login
        // shell runs, and no backslash but before a `$` or a `"`.
        assert_eq!(command.matches('\'').count(), 2);
        let mut chars = command.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                assert!(matches!(chars.peek(), Some('$' | '"')), "{command}");
            }
        }
        assert!(upload_command("0.9.0-0123456789ab").ends_with(&format!(
            "\" perch-upload 0.9.0-0123456789ab {VERSION} {PROTOCOL}'"
        )));
    }

    #[test]
    fn perchd_s_version_line_is_read() {
        assert_eq!(
            parse_version("perchd 0.9.0 (protocol 4)\n"),
            Some(("0.9.0".to_string(), 4))
        );
        assert_eq!(parse_version("perchd 0.9.0"), None);
        assert_eq!(parse_version("bash: ./x: cannot execute binary file"), None);
        let said = upload_failure("perch-version: perchd 0.8.0 (protocol 3)", "halo");
        assert!(said.contains("is 0.8.0 (protocol 3)"), "{said}");
        let said = upload_failure(
            "perch-version: sh: 1: /home/c/.perch/bin/.perchd-x.12: Exec format error",
            "halo",
        );
        assert!(said.contains("would not run there: sh: 1:"), "{said}");
        assert_eq!(
            upload_failure("perch-upload: could not make /x/bin", "halo"),
            "Couldn't install perchd on halo: could not make /x/bin."
        );
    }

    #[test]
    fn a_download_is_checked_against_sha256sums() {
        let tarball = b"not really a tarball";
        let sums = format!(
            "{}  Perch-macos-arm64.tar.gz\n{} *perchd-linux-x86_64.tar.gz\n",
            hex(b"other"),
            hex(tarball)
        );
        assert_eq!(
            check_sum(&sums, "perchd-linux-x86_64.tar.gz", tarball),
            Ok(())
        );
        let damaged = check_sum(&sums, "perchd-linux-x86_64.tar.gz", b"changed").unwrap_err();
        assert!(damaged.contains("does not match"), "{damaged}");
        let unlisted = check_sum(&sums, "perchd-linux-arm64.tar.gz", tarball).unwrap_err();
        assert!(unlisted.contains("does not list"), "{unlisted}");
        // As sha256sum writes it on Linux, two spaces.
        let sums = format!("{}  perchd-linux-arm64.tar.gz", hex(tarball).to_uppercase());
        assert_eq!(
            check_sum(&sums, "perchd-linux-arm64.tar.gz", tarball),
            Ok(())
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

    /// One tar entry, as ustar writes it.
    fn entry(name: &str, kind: u8, data: &[u8]) -> Vec<u8> {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..107].copy_from_slice(b"0000755");
        header[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        header[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        let mut out = header.to_vec();
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
        out
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn perchd_is_taken_out_of_a_tarball_and_hashed_as_the_binary() {
        let binary = elf(62);
        let mut tar = entry("._perchd", b'0', b"apple double");
        tar.extend(entry("dist/", b'5', b""));
        tar.extend(entry("dist/perchd", b'0', &binary));
        tar.extend([0u8; 1024]);
        let tarball = gzip(&tar);
        assert_eq!(extract_perchd(&tarball).unwrap(), binary);
        assert_eq!(binary_in(tarball.clone()).unwrap(), binary);
        assert_eq!(binary_in(binary.clone()).unwrap(), binary);

        // A tarball and the binary in it are the same build.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("perchd.tar.gz"), &tarball).unwrap();
        std::fs::write(dir.path().join("perchd"), &binary).unwrap();
        let (from_tarball, from_binary) = (
            read_file(&dir.path().join("perchd.tar.gz")).unwrap(),
            read_file(&dir.path().join("perchd")).unwrap(),
        );
        assert_eq!(from_tarball.build, from_binary.build);
        assert_eq!(from_tarball.build, file_build(&binary));

        let mut empty = entry("README", b'0', b"hi");
        empty.extend([0u8; 1024]);
        assert!(extract_perchd(&gzip(&empty)).is_err());
        let mac = binary_in(b"\xcf\xfa\xed\xfe macho".to_vec()).unwrap_err();
        assert!(mac.contains("not a Linux binary"), "{mac}");
        assert!(read_file(&dir.path().join("nothing")).is_err());
        assert_eq!(elf_arch(&elf(183)), Ok(Arch::Arm64));
        assert!(elf_arch(&elf(40)).is_err());
    }

    /// A tarball made by the system's own tar, as the release's is, reads.
    #[test]
    fn a_tarball_from_tar_reads() {
        let dir = tempfile::tempdir().unwrap();
        let binary = elf(183);
        std::fs::write(dir.path().join("perchd"), &binary).unwrap();
        let made = std::process::Command::new("tar")
            .arg("-czf")
            .arg(dir.path().join("perchd-linux-arm64.tar.gz"))
            .arg("-C")
            .arg(dir.path())
            .arg("perchd")
            .status();
        if !made.is_ok_and(|status| status.success()) {
            eprintln!("no tar to make a tarball with");
            return;
        }
        let tarball = std::fs::read(dir.path().join("perchd-linux-arm64.tar.gz")).unwrap();
        assert_eq!(extract_perchd(&tarball).unwrap(), binary);
    }

    /// The shells a login shell may be that are here: bash and zsh on
    /// macOS, fish when installed.
    pub(crate) fn shells() -> Vec<String> {
        ["/bin/bash", "/bin/zsh", "/bin/sh"]
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
    /// with a stand-in for perchd on stdin: one that names the version and
    /// protocol expected is put in place, owner-only, and one that names
    /// others is not, and leaves nothing behind.
    #[test]
    fn the_upload_script_installs_only_the_version_expected() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        for shell in shells() {
            let home = tempfile::tempdir().unwrap();
            let upload = |binary: &str, build: &str| {
                let mut child = std::process::Command::new(&shell)
                    .arg("-c")
                    .arg(upload_command(build))
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
                    .write_all(binary.as_bytes())
                    .unwrap();
                child.wait_with_output().unwrap()
            };
            let bin = home.path().join(".perch/bin");
            let good = format!("#!/bin/sh\necho \"perchd {VERSION} (protocol {PROTOCOL})\"\n");
            let done = upload(&good, "0.9.0-0123456789ab");
            assert!(
                done.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&done.stderr)
            );
            let installed = bin.join("perchd-0.9.0-0123456789ab");
            assert_eq!(std::fs::read_to_string(&installed).unwrap(), good);
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&installed), 0o700, "{shell}");
            assert_eq!(mode(&bin), 0o700, "{shell}");
            assert!(String::from_utf8_lossy(&done.stdout).contains("perch-installed:"));

            let old = "#!/bin/sh\necho \"perchd 0.1.0 (protocol 1)\"\n";
            let refused = upload(old, "0.1.0");
            assert!(!refused.status.success(), "{shell}");
            let said = String::from_utf8_lossy(&refused.stderr);
            let last = said.lines().last().unwrap_or_default();
            assert_eq!(last, "perch-version: perchd 0.1.0 (protocol 1)", "{shell}");
            assert!(upload_failure(last, "halo").contains("is 0.1.0 (protocol 1)"));
            let left: Vec<_> = std::fs::read_dir(&bin)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(left, ["perchd-0.9.0-0123456789ab"], "{shell}");
        }
    }

    /// The staging script, run as ssh would have each login shell run it
    /// over a stand-in for perchd on the host: it is copied into place
    /// under the build its bytes name, owner-only, once, a rebuilt one
    /// under another, and one of another version or one that is not there
    /// is not, and leaves nothing behind.
    #[test]
    fn the_staging_script_installs_the_host_s_own_perchd_by_its_build() {
        use std::os::unix::fs::PermissionsExt;
        for shell in shells() {
            let home = tempfile::tempdir().unwrap();
            let built = home.path().join("code/perch/target/debug");
            std::fs::create_dir_all(&built).unwrap();
            let write = |text: &str| {
                let path = built.join("perchd");
                std::fs::write(&path, text).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            };
            let stage = |path: &str| {
                std::process::Command::new(&shell)
                    .arg("-c")
                    .arg(stage_command(&host_path(path).unwrap()))
                    .env_clear()
                    .env("HOME", home.path())
                    .env("SHELL", &shell)
                    .env("PATH", "/usr/bin:/bin:/sbin")
                    .output()
                    .unwrap()
            };
            let staged = |output: &std::process::Output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find_map(|line| line.strip_prefix(STAGED_SAID))
                    .map(|build| build.trim().to_string())
            };
            let bin = home.path().join(".perch/bin");
            let names = || {
                let mut names: Vec<_> = std::fs::read_dir(&bin)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                names
            };

            let good = format!("#!/bin/sh\necho \"perchd {VERSION} (protocol {PROTOCOL})\"\n");
            write(&good);
            let done = stage("~/code/perch/target/debug/perchd");
            assert!(
                done.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&done.stderr)
            );
            let build = file_build(good.as_bytes());
            assert_eq!(staged(&done), Some(build.clone()), "{shell}");
            let installed = bin.join(format!("perchd-{build}"));
            assert_eq!(std::fs::read_to_string(&installed).unwrap(), good);
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&installed), 0o700, "{shell}");
            assert_eq!(mode(&bin), 0o700, "{shell}");

            // Again, by its full path: nothing is copied a second time.
            let before = std::fs::metadata(&installed).unwrap().modified().unwrap();
            let again = stage(&built.join("perchd").display().to_string());
            assert_eq!(staged(&again), Some(build.clone()), "{shell}");
            assert_eq!(
                std::fs::metadata(&installed).unwrap().modified().unwrap(),
                before
            );

            // Rebuilt: another build, beside the first.
            let rebuilt = format!("{good}# rebuilt\n");
            write(&rebuilt);
            let next = file_build(rebuilt.as_bytes());
            assert_eq!(
                staged(&stage("code/perch/target/debug/perchd")),
                Some(next.clone())
            );
            let mut both = [format!("perchd-{build}"), format!("perchd-{next}")];
            both.sort();
            assert_eq!(names(), both, "{shell}");

            // Another version's is refused, and so is a file not there.
            write("#!/bin/sh\necho \"perchd 0.1.0 (protocol 1)\"\n");
            let refused = stage("~/code/perch/target/debug/perchd");
            assert!(!refused.status.success(), "{shell}");
            let said = String::from_utf8_lossy(&refused.stderr);
            let last = said.lines().last().unwrap_or_default();
            assert_eq!(last, "perch-version: perchd 0.1.0 (protocol 1)", "{shell}");
            assert!(stage_failure(last, "halo").contains("is perchd 0.1.0 (protocol 1)"));
            let missing = stage("~/nowhere/perchd");
            assert!(!missing.status.success(), "{shell}");
            let said = String::from_utf8_lossy(&missing.stderr);
            assert!(
                said.lines()
                    .last()
                    .unwrap_or_default()
                    .starts_with("perch-stage: there is no perchd at "),
                "{shell}: {said}"
            );
            assert_eq!(names().len(), 2, "{shell}: {:?}", names());
        }
    }

    #[test]
    fn a_host_path_is_from_home_or_whole_and_plain() {
        assert_eq!(
            host_path("~/code/perch/target/debug/perchd").unwrap(),
            "code/perch/target/debug/perchd"
        );
        assert_eq!(host_path(" /srv/perchd ").unwrap(), "/srv/perchd");
        assert_eq!(host_path("bin/perchd").unwrap(), "bin/perchd");
        for bad in [
            "",
            "~/my code/perchd",
            "~/a;b",
            "$HOME/perchd",
            "-x",
            "~/a'b",
        ] {
            assert!(host_path(bad).is_err(), "{bad}");
        }
    }

    /// An upload whose script is told to stop while it asks the binary
    /// its version, as when the ssh session ends under it, leaves no
    /// temporary file behind, in any login shell.
    #[test]
    fn an_upload_cut_short_leaves_nothing_behind() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};
        for shell in shells() {
            let home = tempfile::tempdir().unwrap();
            let mut child = std::process::Command::new(&shell)
                .arg("-c")
                .arg(upload_command("0.9.0"))
                .env_clear()
                .env("HOME", home.path())
                .env("SHELL", &shell)
                .env("PATH", "/usr/bin:/bin")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            // Slow to answer, so the script is still waiting on it when told.
            let slow =
                format!("#!/bin/sh\nsleep 2\necho \"perchd {VERSION} (protocol {PROTOCOL})\"\n");
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(slow.as_bytes()).unwrap();
            drop(stdin);
            let bin = home.path().join(".perch/bin");
            let deadline = Instant::now() + Duration::from_secs(10);
            let written = loop {
                let ready = std::fs::read_dir(&bin)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .any(|entry| {
                        entry.file_name().to_string_lossy().starts_with(".perchd-")
                            && entry.metadata().unwrap().permissions().mode() & 0o100 != 0
                    });
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
            assert!(!child.wait().unwrap().success(), "{shell}");
            let left: Vec<_> = std::fs::read_dir(&bin)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert!(left.is_empty(), "{shell}: left {left:?}");
        }
    }
}
