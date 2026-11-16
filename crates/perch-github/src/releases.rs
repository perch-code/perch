//! A repository's releases: what Perch's updater asks after, the newest
//! build of Perch and the tarball it ships in; and, with no token, Perch's
//! own latest release and the download of a file from one, which is how a
//! host gets its `perchd` and how an app whose engine is on a host updates.

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
    /// Its address on github.com, which a public repository's answers
    /// with no token.
    #[serde(default)]
    pub browser_download_url: String,
}

/// Where Perch's own releases are downloaded from. The repository is
/// public, so no token is sent: one for another account's repositories
/// would be no help, and nothing of the reader's leaves the Mac.
pub const PERCH_DOWNLOADS: &str = "https://github.com/perch-code/perch/releases/download";

/// The address of `name` in Perch's release of `version` (`0.9.0`).
pub fn perch_download_url(version: &str, name: &str) -> String {
    format!("{PERCH_DOWNLOADS}/v{version}/{name}")
}

/// Where Perch's latest release is asked after with no token.
pub const PERCH_LATEST: &str = "https://api.github.com/repos/perch-code/perch/releases/latest";

/// Why a download did not come.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// Nothing is at the address: a release without that file, or no such
    /// release.
    Missing,
    /// Anything else, said.
    Failed(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Missing => f.write_str("not found"),
            DownloadError::Failed(why) => f.write_str(why),
        }
    }
}

/// A client that sends no token, following redirects (GitHub sends a
/// release's files from another host).
fn anonymous() -> Result<reqwest::Client, DownloadError> {
    reqwest::Client::builder()
        .user_agent(concat!("perch/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| DownloadError::Failed(e.to_string()))
}

/// The release `url` answers with, [`PERCH_LATEST`] in the app, asked
/// with no token. `None` when there is none: GitHub answers a repository
/// with no release with a 404. GitHub allows 60 of these an hour from one
/// address, far more than the updater asks.
pub async fn latest(url: &str) -> Result<Option<Release>, DownloadError> {
    let response = anonymous()?
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| DownloadError::Failed(format!("couldn't ask {url}: {e}")))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(DownloadError::Failed(format!(
            "couldn't ask {url}: the server answered {status}"
        )));
    }
    let body = response
        .text()
        .await
        .map_err(|e| DownloadError::Failed(format!("couldn't ask {url}: {e}")))?;
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|e| DownloadError::Failed(format!("{url} answered something else: {e}")))
}

/// The file at `url`, fetched with no token, following redirects, and
/// refused past `limit` bytes.
pub async fn download(url: &str, limit: u64) -> Result<Vec<u8>, DownloadError> {
    let mut response = anonymous()?
        .get(url)
        .send()
        .await
        .map_err(|e| DownloadError::Failed(format!("couldn't download {url}: {e}")))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(DownloadError::Missing);
    }
    if !status.is_success() {
        return Err(DownloadError::Failed(format!(
            "couldn't download {url}: the server answered {status}"
        )));
    }
    let too_big = || DownloadError::Failed(format!("{url} is larger than {limit} bytes"));
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(too_big());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| DownloadError::Failed(format!("couldn't download {url}: {e}")))?
    {
        if body.len() as u64 + chunk.len() as u64 > limit {
            return Err(too_big());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_perch_download_is_under_its_version_s_tag() {
        assert_eq!(
            perch_download_url("0.9.0", "SHA256SUMS"),
            "https://github.com/perch-code/perch/releases/download/v0.9.0/SHA256SUMS"
        );
    }
}
