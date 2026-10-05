//! A repository's releases: what Perch's updater asks after, the newest
//! build of Perch and the tarball it ships in.

use serde::Deserialize;

/// One release, as `GET /repos/{owner}/{repo}/releases/latest` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    /// The tag it was cut from, `v0.2.0`.
    pub tag_name: String,
    /// Its page on GitHub, with the notes.
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

impl Release {
    /// The download named `name`, if the release has one.
    pub fn asset(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

/// A file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReleaseAsset {
    pub id: u64,
    pub name: String,
    /// In bytes.
    pub size: u64,
    /// The API's address for it, which answers `application/octet-stream`
    /// with a redirect to the file. A private repository's downloads are
    /// only reachable this way, with the token.
    pub url: String,
}
