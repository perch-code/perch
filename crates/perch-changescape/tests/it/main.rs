//! The crate's tests on the shared conformance fixtures
//! (`tests/fixtures/conformance/`, copied from the Changescape repo's
//! `skill/src/changescape/contract/`, in page format 4), as one test binary.

mod conformance;
mod lookup;

use std::path::{Path, PathBuf};

pub const REPO: &str = "calc/calc";
pub const HEAD: &str = "3e81042d07e4d404b84526777e768a456a6a4fa9";

pub fn cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance")
}

/// Copy case `name` into `root` as pull request 7 at `sha`.
pub fn install(root: &Path, name: &str, sha: &str) -> PathBuf {
    install_patched(root, name, sha, |_| {})
}

/// Copy case `name` into `root` as pull request 7 at `sha`, its page
/// changed by `patch`.
pub fn install_patched(
    root: &Path,
    name: &str,
    sha: &str,
    patch: impl FnOnce(&mut serde_json::Value),
) -> PathBuf {
    let to = perch_changescape::build_dir(root, REPO, 7, sha);
    std::fs::create_dir_all(&to).unwrap();
    std::fs::copy(
        cases().join(name).join("status.json"),
        to.join("status.json"),
    )
    .unwrap();
    let mut page: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cases().join(name).join("page.json")).unwrap())
            .unwrap();
    patch(&mut page);
    std::fs::write(to.join("page.json"), page.to_string()).unwrap();
    to
}
