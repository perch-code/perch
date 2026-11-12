//! Update: Perch replacing itself with its latest release. This is the
//! part with no window: which release is newer, where the running app
//! lives, unpacking a download beside it and swapping it in. The backend
//! asks GitHub and downloads (`backend::update`); the drawer's footer
//! offers the restart.
//!
//! A release's tarball comes down through the API, not a browser, so
//! macOS never quarantines it and Gatekeeper never asks about an app that
//! isn't notarized.
//!
//! On macOS the app is `Perch.app`. On Linux it's a `perch` folder
//! holding the binary and a `VERSION` file, which says what it is the way
//! the bundle's Info.plist does.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use semver::Version;

/// The release's file Perch installs from: the app for this platform and
/// processor, tarred.
pub const ASSET: &str = if cfg!(target_os = "macos") {
    if cfg!(target_arch = "x86_64") {
        "Perch-macos-x86_64.tar.gz"
    } else {
        "Perch-macos-arm64.tar.gz"
    }
} else if cfg!(target_arch = "x86_64") {
    "Perch-linux-x86_64.tar.gz"
} else {
    "Perch-linux-arm64.tar.gz"
};

/// The folder a release unpacks to.
const BUNDLE: &str = if cfg!(target_os = "macos") {
    "Perch.app"
} else {
    "perch"
};

/// The file in a bundle that says it's whole and which version it is.
const MANIFEST: &str = if cfg!(target_os = "macos") {
    "Contents/Info.plist"
} else {
    "VERSION"
};

/// macOS's tar by its path. NixOS has no /usr/bin/tar, so elsewhere it's
/// whichever tar is on PATH.
const TAR: &str = if cfg!(target_os = "macos") {
    "/usr/bin/tar"
} else {
    "tar"
};

/// What the updater has to offer the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Nothing newer, or not looked yet.
    Current,
    /// A newer release, unpacked and checked, waiting for a restart.
    Ready { version: String },
    /// The restart is under way: swapping the app, then quitting.
    Installing { version: String },
    /// The swap failed, and the app is as it was.
    Failed { version: String, error: String },
}

/// What a look the reader asked for found, for the window they asked in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// A newer release is coming down, which can take a while.
    Downloading { version: String },
    /// This build is the latest.
    Current,
    /// A newer release is unpacked and waiting for a restart.
    Ready { version: String },
    /// This build isn't an app Perch can replace: `cargo run`, say.
    NotARelease,
    /// The look failed.
    Failed(String),
}

/// The version this build is.
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml's version is semver")
}

/// The version a release's tag names, when it's newer than `current`.
pub fn newer(tag: &str, current: &Version) -> Option<Version> {
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()?;
    (version > *current).then_some(version)
}

/// The app this process runs from, when it runs from one that can be
/// replaced. None under `cargo run`, and in a copy macOS translocated,
/// which is read-only and gone at quit.
pub fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    bundle_of(&exe)
}

#[cfg(target_os = "macos")]
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let bundle = exe.parent()?.parent()?.parent()?;
    let shaped = exe.parent()?.file_name()? == "MacOS"
        && exe.parent()?.parent()?.file_name()? == "Contents"
        && bundle.extension()? == "app";
    let translocated = bundle
        .components()
        .any(|c| c.as_os_str() == "AppTranslocation");
    (shaped && !translocated).then(|| bundle.to_path_buf())
}

/// The binary's folder, when it has a release's `VERSION` beside it. A
/// cargo target dir and `~/.cargo/bin` have none.
#[cfg(not(target_os = "macos"))]
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let bundle = exe.parent()?;
    (exe.file_name()? == "perch" && bundle.join(MANIFEST).is_file()).then(|| bundle.to_path_buf())
}

/// Where downloads are unpacked: `~/.perch/updates`, one directory per
/// version.
pub fn updates_dir() -> PathBuf {
    perch_github::config::perch_home().join("updates")
}

/// The app unpacked for `version`, if it's there.
pub fn staged(dir: &Path, version: &Version) -> Option<PathBuf> {
    let app = dir.join(version.to_string()).join(BUNDLE);
    app.join(MANIFEST).is_file().then_some(app)
}

/// Remove every unpacked version but `keep`: older ones, ones already
/// installed, half-written ones.
pub fn prune(dir: &Path, keep: Option<&Version>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let keep = keep.map(Version::to_string);
    for entry in entries.flatten() {
        if keep.as_deref() != entry.file_name().to_str() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Unpack a downloaded tarball into `dir/<version>/` and check it: it
/// says it's `version` and Perch, and on macOS its signature holds. A
/// failed check leaves nothing behind.
pub fn stage(dir: &Path, version: &Version, tarball: &[u8]) -> Result<PathBuf, String> {
    let into = dir.join(version.to_string());
    let _ = std::fs::remove_dir_all(&into);
    let result = unpack(&into, version, tarball);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&into);
    }
    result
}

fn unpack(into: &Path, version: &Version, tarball: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(into).map_err(|e| format!("couldn't make {}: {e}", into.display()))?;
    let archive = into.join(ASSET);
    std::fs::write(&archive, tarball).map_err(|e| format!("couldn't save the download: {e}"))?;
    run(Command::new(TAR)
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(into))?;
    let _ = std::fs::remove_file(&archive);

    let app = into.join(BUNDLE);
    let said = version_of(&app)?;
    if said != version.to_string() {
        return Err(format!("the download says it's {said}, not {version}"));
    }
    check(&app)?;
    Ok(app)
}

/// The version the app at `bundle` says it is.
#[cfg(target_os = "macos")]
fn version_of(bundle: &Path) -> Result<String, String> {
    plist_value(&bundle.join(MANIFEST), "CFBundleShortVersionString")
}

#[cfg(not(target_os = "macos"))]
fn version_of(bundle: &Path) -> Result<String, String> {
    std::fs::read_to_string(bundle.join(MANIFEST))
        .map(|text| text.trim().to_string())
        .map_err(|e| format!("the download has no {MANIFEST}: {e}"))
}

/// That the app at `bundle` is Perch and whole: its identifier, and its
/// signature.
#[cfg(target_os = "macos")]
fn check(bundle: &Path) -> Result<(), String> {
    let id = plist_value(&bundle.join(MANIFEST), "CFBundleIdentifier")?;
    if id != BUNDLE_ID {
        return Err(format!("the download is {id}, not Perch"));
    }
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(bundle))
}

/// That the app at `bundle` has a binary to run. There's no signature to
/// check; tar has already refused a tarball that came down damaged.
#[cfg(not(target_os = "macos"))]
fn check(bundle: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let runnable = std::fs::metadata(bundle.join("perch"))
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
    if runnable {
        Ok(())
    } else {
        Err("the download has no perch binary".into())
    }
}

#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "com.perch.perch";

#[cfg(target_os = "macos")]
fn plist_value(plist: &Path, key: &str) -> Result<String, String> {
    let out = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist)
        .output()
        .map_err(|e| format!("couldn't run plutil: {e}"))?;
    if !out.status.success() {
        return Err(format!("the download's Info.plist has no {key}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn run(command: &mut Command) -> Result<(), String> {
    let out = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run {:?}: {e}", command.get_program()))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(format!(
        "{} failed: {}",
        Path::new(command.get_program()).display(),
        stderr.trim()
    ))
}

/// Put `staged` where `bundle` is. The running process keeps its own
/// files open, so the old app can move aside under it; it's deleted once
/// the new one is in place, or moved back if that fails.
///
/// The new app goes next to the old one first, so the swap is two
/// renames in one directory. `~/.perch` and the app's folder are usually
/// on one volume and the first is a rename too; when they're not, it's a
/// copy.
pub fn install(staged: &Path, bundle: &Path) -> Result<(), String> {
    let parent = bundle
        .parent()
        .ok_or_else(|| format!("{} has no folder", bundle.display()))?;
    let beside = |what: &str| {
        parent.join(if cfg!(target_os = "macos") {
            format!(".Perch-{what}.app")
        } else {
            format!(".perch-{what}")
        })
    };
    let incoming = beside("update");
    let outgoing = beside("previous");
    let _ = std::fs::remove_dir_all(&incoming);
    let _ = std::fs::remove_dir_all(&outgoing);

    if std::fs::rename(staged, &incoming).is_err() {
        let mut copy = if cfg!(target_os = "macos") {
            Command::new("/usr/bin/ditto")
        } else {
            let mut cp = Command::new("cp");
            cp.arg("-a");
            cp
        };
        run(copy.arg(staged).arg(&incoming)).map_err(|e| {
            let _ = std::fs::remove_dir_all(&incoming);
            format!("couldn't copy the update next to {}: {e}", bundle.display())
        })?;
    }
    if let Err(e) = std::fs::rename(bundle, &outgoing) {
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(format!("couldn't move {} aside: {e}", bundle.display()));
    }
    if let Err(e) = std::fs::rename(&incoming, bundle) {
        let _ = std::fs::rename(&outgoing, bundle);
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(format!(
            "couldn't put the update at {}: {e}",
            bundle.display()
        ));
    }
    let _ = std::fs::remove_dir_all(&outgoing);
    Ok(())
}

/// Start `bundle` once this process has exited. `open` on an app that's
/// still running only brings it forward, so a shell outside Perch waits
/// for this pid to go first. The caller quits after.
pub fn relaunch_after_exit(bundle: &Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let start = if cfg!(target_os = "macos") {
        r#"exec /usr/bin/open "$2""#
    } else {
        r#"exec "$2/perch""#
    };
    Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; {start}"#
        ))
        .arg("perch-relaunch")
        .arg(std::process::id().to_string())
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn only_a_later_tag_is_newer() {
        assert_eq!(newer("v0.2.0", &v("0.1.0")), Some(v("0.2.0")));
        assert_eq!(newer("0.2.0", &v("0.1.0")), Some(v("0.2.0")));
        assert_eq!(newer("v0.1.0", &v("0.1.0")), None);
        assert_eq!(newer("v0.1.0", &v("0.2.0")), None);
        assert_eq!(newer("v0.2.0", &v("0.2.0-beta.1")), Some(v("0.2.0")));
        assert_eq!(newer("nightly", &v("0.1.0")), None);
    }

    #[test]
    fn the_asset_is_the_build_for_this_platform_and_processor() {
        let arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            other => other,
        };
        let os = match std::env::consts::OS {
            "macos" => "macos",
            _ => "linux",
        };
        assert_eq!(ASSET, format!("Perch-{os}-{arch}.tar.gz"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_bundle_is_three_folders_up_from_the_binary() {
        assert_eq!(
            bundle_of(Path::new("/Applications/Perch.app/Contents/MacOS/perch")),
            Some(PathBuf::from("/Applications/Perch.app"))
        );
        assert_eq!(
            bundle_of(Path::new("/Users/me/perch/target/release/perch")),
            None
        );
        assert_eq!(
            bundle_of(Path::new(
                "/private/var/folders/x/AppTranslocation/1234/d/Perch.app/Contents/MacOS/perch"
            )),
            None
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn the_bundle_is_the_binarys_folder_when_it_has_a_version() {
        let tmp = tempfile::tempdir().unwrap();
        let installed = tmp.path().join("share/perch");
        fake_app(&installed, "0.1.0");
        assert_eq!(bundle_of(&installed.join("perch")), Some(installed));

        let built = tmp.path().join("target/release");
        std::fs::create_dir_all(&built).unwrap();
        std::fs::write(built.join("perch"), "").unwrap();
        assert_eq!(bundle_of(&built.join("perch")), None);
    }

    /// A Perch.app with just an Info.plist, ad-hoc signed.
    #[cfg(target_os = "macos")]
    fn fake_app(at: &Path, version: &str) {
        std::fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
        std::fs::write(
            at.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
<key>CFBundleExecutable</key><string>perch</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
            ),
        )
        .unwrap();
        std::fs::copy("/usr/bin/true", at.join("Contents/MacOS/perch")).unwrap();
        run(Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(at))
        .unwrap();
    }

    /// A perch folder with a binary and its VERSION.
    #[cfg(not(target_os = "macos"))]
    fn fake_app(at: &Path, version: &str) {
        std::fs::create_dir_all(at).unwrap();
        std::fs::write(at.join(MANIFEST), format!("{version}\n")).unwrap();
        std::fs::copy("/bin/true", at.join("perch")).unwrap();
    }

    fn tarball_of(app: &Path) -> Vec<u8> {
        let out = Command::new(TAR)
            .env("COPYFILE_DISABLE", "1")
            .arg("-czf")
            .arg("-")
            .arg("-C")
            .arg(app.parent().unwrap())
            .arg(BUNDLE)
            .output()
            .unwrap();
        assert!(out.status.success());
        out.stdout
    }

    #[test]
    fn a_download_is_unpacked_and_checked() {
        let tmp = tempfile::tempdir().unwrap();
        let built = tmp.path().join("build").join(BUNDLE);
        fake_app(&built, "0.2.0");
        let dir = tmp.path().join("updates");

        let app = stage(&dir, &v("0.2.0"), &tarball_of(&built)).unwrap();
        assert_eq!(app, dir.join("0.2.0").join(BUNDLE));
        assert_eq!(staged(&dir, &v("0.2.0")), Some(app));
        assert!(!dir.join("0.2.0").join(ASSET).exists());
    }

    #[test]
    fn a_download_of_another_version_is_refused_and_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let built = tmp.path().join("build").join(BUNDLE);
        fake_app(&built, "0.1.5");
        let dir = tmp.path().join("updates");

        let error = stage(&dir, &v("0.2.0"), &tarball_of(&built)).unwrap_err();
        assert!(error.contains("0.1.5"), "{error}");
        assert!(!dir.join("0.2.0").exists());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_tampered_download_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let built = tmp.path().join("build").join(BUNDLE);
        fake_app(&built, "0.2.0");
        std::fs::write(built.join("Contents/MacOS/perch"), b"not what was signed").unwrap();
        let dir = tmp.path().join("updates");

        assert!(stage(&dir, &v("0.2.0"), &tarball_of(&built)).is_err());
        assert!(!dir.join("0.2.0").exists());
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn a_download_without_its_binary_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let built = tmp.path().join("build").join(BUNDLE);
        fake_app(&built, "0.2.0");
        std::fs::remove_file(built.join("perch")).unwrap();
        let dir = tmp.path().join("updates");

        assert!(stage(&dir, &v("0.2.0"), &tarball_of(&built)).is_err());
        assert!(!dir.join("0.2.0").exists());
    }

    #[test]
    fn install_swaps_the_app_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("Applications").join(BUNDLE);
        fake_app(&bundle, "0.1.0");
        let dir = tmp.path().join("updates");
        let built = tmp.path().join("build").join(BUNDLE);
        fake_app(&built, "0.2.0");
        let app = stage(&dir, &v("0.2.0"), &tarball_of(&built)).unwrap();

        install(&app, &bundle).unwrap();
        assert_eq!(version_of(&bundle).unwrap(), "0.2.0");
        let left: Vec<_> = std::fs::read_dir(bundle.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, [BUNDLE]);
        assert_eq!(staged(&dir, &v("0.2.0")), None);
    }

    #[test]
    fn a_failed_install_leaves_the_app_as_it_was() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("Applications").join(BUNDLE);
        fake_app(&bundle, "0.1.0");

        assert!(install(&tmp.path().join("missing").join(BUNDLE), &bundle).is_err());
        assert_eq!(version_of(&bundle).unwrap(), "0.1.0");
    }

    #[test]
    fn prune_keeps_only_the_version_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        for version in ["0.1.0", "0.2.0", "0.3.0"] {
            std::fs::create_dir_all(tmp.path().join(version)).unwrap();
        }
        prune(tmp.path(), Some(&v("0.3.0")));
        let left: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["0.3.0"]);
    }
}
