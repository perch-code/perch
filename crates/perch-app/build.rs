//! Builds the engines this app carries, and puts them in it.
//!
//! The app runs its own `perchd` on whatever host it is pointed at, so it
//! has one for each system and processor Perch runs on: Linux and macOS,
//! on x86_64 and arm64. This builds all four from the same source as the
//! app, gzips them into `OUT_DIR`, and writes `engines.rs`, which `main.rs`
//! includes: for each, its platform, the SHA-256 of the binary and the
//! bytes. The app itself is built for this machine only.
//!
//! - A Linux `perchd` is built against musl, statically, through `cargo
//!   zigbuild`, as a release's is: zig is the C compiler and linker for it
//!   from any machine. With no cargo-zigbuild, on Linux, this machine's
//!   own processor's is built with `musl-gcc` when that is there.
//! - A macOS `perchd` is built by the toolchain itself on a Mac, for either
//!   processor. Anywhere else it needs Apple's SDK: it is built through
//!   zig when `SDKROOT` names one, and left out otherwise.
//! - One whose tools are not here is left out, with a warning that says
//!   what to install, and the app is built with the rest. For a host it
//!   carries none for, the app builds `perchd` from this source when it
//!   connects (see `perch_engine::wire::provision`). One that does not
//!   compile fails the build, as the app's own source would.
//! - They are built with the app's profile, into `engines` under the
//!   target directory: cargo holds a lock on the directory it is building
//!   the app in, and a cargo started from here would wait on it for ever.
//!
//! `PERCH_ENGINES` says otherwise:
//!
//! - A directory: the four `perchd-<linux|macos>-<x86_64|arm64>.tar.gz`
//!   in it are taken as they are, and nothing is built. A release does
//!   this, with each built on its own runner. All four have to be there.
//! - `none`: the app carries none, and nothing is built: for working on
//!   the app alone, or a check that should be quick.
//!
//! Under clippy and rust-analyzer nothing is built either: neither makes
//! an app to run.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

/// Each platform Perch has a `perchd` for, as the app names it, and the
/// Rust target it is built as.
const PLATFORMS: [(&str, &str); 4] = [
    ("linux-x86_64", "x86_64-unknown-linux-musl"),
    ("linux-arm64", "aarch64-unknown-linux-musl"),
    ("macos-x86_64", "x86_64-apple-darwin"),
    ("macos-arm64", "aarch64-apple-darwin"),
];

/// An engine ready to go into the app.
struct Engine {
    key: &'static str,
    /// The SHA-256 of the binary, in hex.
    sum: String,
    /// The binary, gzipped, in `OUT_DIR`.
    packed: PathBuf,
}

/// Why there is no engine for a platform.
enum Missed {
    /// Its tools are not here: what to install.
    LeftOut(String),
    /// It did not build: what cargo said.
    Failed(String),
}

fn main() {
    for name in [
        "PERCH_ENGINES",
        "SDKROOT",
        "RUSTC_WORKSPACE_WRAPPER",
        "RA_RUSTC_WRAPPER",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let given = env::var("PERCH_ENGINES").unwrap_or_default();
    let mut engines = Vec::new();
    let mut left_out = Vec::new();
    if given == "none" || (given.is_empty() && linting()) {
        // None, and nothing to say: it was asked for.
    } else if !given.is_empty() {
        // A build script runs in its package's folder, not where cargo
        // was started, so a path from there would be looked for here.
        if !Path::new(&given).is_absolute() {
            fail(&format!(
                "PERCH_ENGINES, {given}, has to be a whole path, from /, or none"
            ));
        }
        for (key, _) in PLATFORMS {
            match given_engine(Path::new(&given), key, &out) {
                Ok(engine) => engines.push(engine),
                Err(why) => fail(&format!("PERCH_ENGINES, {given}: {why}")),
            }
        }
    } else {
        let build = Build::here(&out);
        build.watch_source();
        for (key, triple) in PLATFORMS {
            match build.engine(key, triple) {
                Ok(binary) => match pack(key, &binary, &out, build.profile == "dev") {
                    Ok(engine) => engines.push(engine),
                    Err(why) => fail(&format!("perchd for {key}: {why}")),
                },
                Err(Missed::LeftOut(why)) => left_out.push(format!("{key} ({why})")),
                Err(Missed::Failed(why)) => {
                    fail(&format!("perchd for {key} did not build:\n{why}"))
                }
            }
        }
    }
    if let Err(error) = fs::write(out.join("engines.rs"), engines_rs(&engines)) {
        fail(&format!("could not write engines.rs: {error}"));
    }
    if !left_out.is_empty() {
        println!(
            "cargo:warning=this Perch carries no perchd for {}",
            left_out.join("; ")
        );
    }
}

/// Stop the build, saying why.
fn fail(why: &str) -> ! {
    eprintln!("error: {why}");
    std::process::exit(1);
}

/// Whether this is a run that makes no app: clippy's, or the one
/// rust-analyzer makes to read what build scripts say.
fn linting() -> bool {
    env::var_os("RA_RUSTC_WRAPPER").is_some()
        || env::var_os("RUSTC_WORKSPACE_WRAPPER").is_some_and(|wrapper| {
            Path::new(&wrapper)
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains("clippy"))
        })
}

/// What `main.rs` includes: the engines, each as its platform, its hash
/// and its bytes.
fn engines_rs(engines: &[Engine]) -> String {
    let mut text = String::from(
        "/// The engines this app carries: for each, its platform, the SHA-256 of\n\
         /// its `perchd`, and that `perchd` gzipped. Written by build.rs.\n\
         pub(crate) static ENGINES: &[(&str, &str, &[u8])] = &[\n",
    );
    for engine in engines {
        text.push_str(&format!(
            "    ({:?}, {:?}, include_bytes!({:?})),\n",
            engine.key,
            engine.sum,
            engine.packed.display().to_string()
        ));
    }
    text.push_str("];\n");
    text
}

/// When `path` was last written.
fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

/// Gzip `binary` into `out` and take its SHA-256, unless that was done
/// since it was last built. Quickly rather than small for a dev build,
/// whose `perchd` is several times a release's and built again often.
fn pack(key: &'static str, binary: &Path, out: &Path, quick: bool) -> Result<Engine, String> {
    let packed = out.join(format!("perchd-{key}.gz"));
    let sum_file = out.join(format!("perchd-{key}.sha256"));
    let kept = match (modified(binary), modified(&packed), modified(&sum_file)) {
        (Some(built), Some(packed), Some(summed)) if packed > built && summed > built => {
            fs::read_to_string(&sum_file)
                .ok()
                .map(|sum| sum.trim().to_string())
                .filter(|sum| sum.len() == 64)
        }
        _ => None,
    };
    let sum = match kept {
        Some(sum) => sum,
        None => {
            let bytes = fs::read(binary)
                .map_err(|e| format!("could not read {}: {e}", binary.display()))?;
            let sum: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let level = if quick {
                flate2::Compression::fast()
            } else {
                flate2::Compression::default()
            };
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), level);
            let gzipped = encoder
                .write_all(&bytes)
                .and_then(|()| encoder.finish())
                .map_err(|e| format!("could not gzip {}: {e}", binary.display()))?;
            // The hash last: it is what says the two are a pair.
            fs::write(&packed, gzipped)
                .and_then(|()| fs::write(&sum_file, &sum))
                .map_err(|e| format!("could not write {}: {e}", packed.display()))?;
            sum
        }
    };
    Ok(Engine { key, sum, packed })
}

/// The engine for `key` out of `dir`, a release's tarball of it there.
fn given_engine(dir: &Path, key: &'static str, out: &Path) -> Result<Engine, String> {
    let tarball = dir.join(format!("perchd-{key}.tar.gz"));
    println!("cargo:rerun-if-changed={}", tarball.display());
    if !tarball.is_file() {
        return Err(format!("there is no {}", tarball.display()));
    }
    let unpacked = out.join(format!("given-{key}"));
    let _ = fs::remove_dir_all(&unpacked);
    fs::create_dir_all(&unpacked)
        .map_err(|e| format!("could not make {}: {e}", unpacked.display()))?;
    let untarred = Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&unpacked)
        .output()
        .map_err(|e| format!("could not run tar: {e}"))?;
    if !untarred.status.success() {
        return Err(format!(
            "tar could not unpack {}: {}",
            tarball.display(),
            String::from_utf8_lossy(&untarred.stderr).trim()
        ));
    }
    let binary = unpacked.join("perchd");
    if !binary.is_file() {
        return Err(format!("{} has no perchd in it", tarball.display()));
    }
    pack(key, &binary, out, false)
}

/// What building the engines here goes by.
struct Build {
    workspace: PathBuf,
    /// The app's profile, which the engines are built with too.
    profile: String,
    /// Where cargo puts that profile's files.
    profile_dir: String,
    /// The target directory the engines are built in: one of their own.
    engines_dir: PathBuf,
}

impl Build {
    /// From where cargo is building the app: `OUT_DIR` is
    /// `<target>/[<triple>/]<profile>/build/<package>-<hash>/out`.
    fn here(out: &Path) -> Build {
        let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("a manifest"));
        let workspace = manifest
            .ancestors()
            .nth(2)
            .expect("the app is two folders into its workspace")
            .to_path_buf();
        let profile_at = out.ancestors().nth(3).expect("OUT_DIR is in a profile's");
        let profile_dir = profile_at
            .file_name()
            .expect("a profile's folder has a name")
            .to_string_lossy()
            .into_owned();
        let above = profile_at.parent().expect("a profile's folder is in one");
        let triple = env::var_os("TARGET").unwrap_or_default();
        let target_dir = match above.file_name() {
            Some(name) if name == triple.as_os_str() => above.parent().unwrap_or(above),
            _ => above,
        };
        Build {
            workspace,
            profile: if profile_dir == "debug" {
                "dev".to_string()
            } else {
                profile_dir.clone()
            },
            profile_dir,
            engines_dir: target_dir.join("engines"),
        }
    }

    /// Run again when what `perchd` is built from changes: every crate but
    /// the app, and what says how they are built.
    fn watch_source(&self) {
        let crates = self.workspace.join("crates");
        for entry in fs::read_dir(&crates).into_iter().flatten().flatten() {
            if entry.file_name() != "perch-app" {
                println!("cargo:rerun-if-changed={}", entry.path().display());
            }
        }
        for file in [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            ".cargo/config.toml",
        ] {
            println!(
                "cargo:rerun-if-changed={}",
                self.workspace.join(file).display()
            );
        }
    }

    /// Build `perchd` for `key`, the target `triple`, and give where it is.
    fn engine(&self, key: &str, triple: &str) -> Result<PathBuf, Missed> {
        let (subcommand, musl_gcc) = plan(key).map_err(Missed::LeftOut)?;
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut command = Command::new(cargo);
        command
            .current_dir(&self.workspace)
            .args([subcommand, "-p", "perchd", "--target", triple, "--profile"])
            .arg(&self.profile)
            .arg("--target-dir")
            .arg(&self.engines_dir)
            .env("PATH", path_with_cargo_bin());
        for (name, _) in env::vars_os() {
            if of_this_build(&name.to_string_lossy()) {
                command.env_remove(name);
            }
        }
        if musl_gcc {
            command.env(format!("CC_{}", triple.replace('-', "_")), "musl-gcc");
        }
        let output = command
            .output()
            .map_err(|e| Missed::Failed(format!("could not run cargo: {e}")))?;
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr);
            return Err(match tool_missing(&said, triple) {
                Some(what) => Missed::LeftOut(what),
                None => Missed::Failed(last_lines(&said, 40)),
            });
        }
        let binary = self
            .engines_dir
            .join(triple)
            .join(&self.profile_dir)
            .join("perchd");
        if binary.is_file() {
            Ok(binary)
        } else {
            Err(Missed::Failed(format!(
                "cargo built it, and there is no {}",
                binary.display()
            )))
        }
    }
}

/// How `perchd` for `key` is built on this machine: cargo's subcommand,
/// and whether `musl-gcc` is its C compiler. Err when what it needs is not
/// here, with what that is.
fn plan(key: &str) -> Result<(&'static str, bool), String> {
    let (os, arch) = key.split_once('-').unwrap_or((key, ""));
    let own_arch = match env::consts::ARCH {
        "aarch64" => arch == "arm64",
        here => arch == here,
    };
    let zigbuild = on_path("cargo-zigbuild");
    match (os, env::consts::OS) {
        ("macos", "macos") => Ok(("build", false)),
        ("macos", _) if zigbuild && env::var_os("SDKROOT").is_some_and(|sdk| !sdk.is_empty()) => {
            Ok(("zigbuild", false))
        }
        ("macos", _) => Err(
            "it is built on a Mac, or here with zig, cargo-zigbuild and SDKROOT naming a macOS SDK"
                .to_string(),
        ),
        (_, _) if zigbuild => Ok(("zigbuild", false)),
        (_, "linux") if own_arch && on_path("musl-gcc") => Ok(("build", true)),
        _ => Err("it needs zig, and cargo install cargo-zigbuild".to_string()),
    }
}

/// What a failed build was missing, when that is why it failed.
fn tool_missing(said: &str, triple: &str) -> Option<String> {
    let lower = said.to_lowercase();
    if lower.contains("no such command: `zigbuild`")
        || lower.contains("no such subcommand: `zigbuild`")
    {
        return Some("it needs zig, and cargo install cargo-zigbuild".to_string());
    }
    if lower.contains("zig")
        && (lower.contains("not found")
            || lower.contains("failed to find")
            || lower.contains("no such file"))
    {
        return Some("cargo-zigbuild found no zig to run: install zig".to_string());
    }
    if said.contains("target may not be installed")
        || said.contains("can't find crate for `core`")
        || said.contains("can't find crate for `std`")
    {
        return Some(format!("it needs rustup target add {triple}"));
    }
    None
}

/// The last `count` lines of `said` that say anything.
fn last_lines(said: &str, count: usize) -> String {
    let lines: Vec<&str> = said
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

/// Whether `name` is something cargo set for building the app, which a
/// cargo building `perchd` for another target must not take as its own:
/// the app's target and profile, its flags (which are this machine's, and
/// would be passed to a compiler for another), and a lint driver in
/// rustc's place.
fn of_this_build(name: &str) -> bool {
    const NAMES: [&str; 19] = [
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_MANIFEST_DIR",
        "CARGO_MANIFEST_PATH",
        "CARGO_MANIFEST_LINKS",
        "CARGO_PRIMARY_PACKAGE",
        "CARGO_CRATE_NAME",
        "CARGO_BIN_NAME",
        "TARGET",
        "HOST",
        "PROFILE",
        "OPT_LEVEL",
        "DEBUG",
        "OUT_DIR",
        "NUM_JOBS",
    ];
    const PREFIXES: [&str; 4] = ["CARGO_FEATURE_", "CARGO_CFG_", "CARGO_PKG_", "DEP_"];
    NAMES.contains(&name) || PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// The directories programs are looked for in: `PATH`, then cargo's own
/// bin, which a build started from an app or an editor may not have on it
/// and where cargo-zigbuild is.
fn search_path() -> Vec<PathBuf> {
    let cargo_bin = match env::var_os("CARGO_HOME").filter(|home| !home.is_empty()) {
        Some(home) => Some(PathBuf::from(home).join("bin")),
        None => env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo/bin")),
    };
    env::var_os("PATH")
        .iter()
        .flat_map(env::split_paths)
        .chain(cargo_bin)
        .collect()
}

fn path_with_cargo_bin() -> OsString {
    env::join_paths(search_path()).unwrap_or_default()
}

/// Whether `program` is there to run.
fn on_path(program: &str) -> bool {
    search_path().iter().any(|dir| dir.join(program).is_file())
}
