//! Update's background half: ask GitHub for Perch's latest release now
//! and then, download and unpack it when it's newer, and swap it in when
//! the reader restarts.
//!
//! The app's own engine does this with the reader's token. An app whose
//! engine is on a host does it itself, with no token (the repository is
//! public), since the host's engine has no app of this Mac's to replace
//! (`wire::remote`). The host's engine updates nothing: the app installs
//! the `perchd` of its own build there on its next connection.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use perch_github::{GitHub, Release, ReleaseAsset};
use semver::Version;

use super::{Ctx, Event, EventSender};
use crate::feedback::REPO;
use crate::update::{self, Checked, UpdateStatus};

/// How long after launch the first look waits: past the token, and past
/// the burst of reads a new window makes.
const FIRST_CHECK: Duration = Duration::from_secs(30);

/// How often to look. Releases are rare, and the look costs a 304.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// How often the clock is read. Tokio's timers stop while the Mac
/// sleeps, so six hours on one could be days of wall time; this wakes
/// often and compares against the wall clock.
const TICK: Duration = Duration::from_secs(15 * 60);

/// Where a look asks after Perch's latest release and downloads it from.
#[derive(Clone)]
pub(crate) enum Releases {
    /// Through the engine's GitHub client, with the reader's token, which
    /// may not be there yet: the app's own engine.
    Token(Arc<dyn Fn() -> Option<Arc<GitHub>> + Send + Sync>),
    /// With no token, `latest` answering with the release: the app whose
    /// engine is on a host, which holds the token.
    Anonymous { latest: String },
}

impl Releases {
    /// With the token `ctx` has when it looks.
    fn of(ctx: &Ctx) -> Releases {
        let ctx = ctx.clone();
        Releases::Token(Arc::new(move || ctx.github()))
    }

    /// Perch's own repository, with no token.
    pub(crate) fn anonymous() -> Releases {
        Releases::Anonymous {
            latest: perch_github::releases::PERCH_LATEST.to_string(),
        }
    }

    /// The latest release. No token is an error, not an answer: it says
    /// nothing of what was downloaded before.
    async fn latest(&self) -> Result<Option<Release>, String> {
        match self {
            Releases::Token(github) => {
                let Some(github) = github() else {
                    return Err("there's no GitHub token to ask with".into());
                };
                github.latest_release(REPO).await.map_err(|e| e.to_string())
            }
            Releases::Anonymous { latest } => perch_github::releases::latest(latest)
                .await
                .map_err(|e| e.to_string()),
        }
    }

    /// `asset`, whole: anything short of the size the release lists is an
    /// error, so a dropped connection never passes for the file.
    async fn download(&self, asset: &ReleaseAsset) -> Result<Vec<u8>, String> {
        match self {
            Releases::Token(github) => {
                let Some(github) = github() else {
                    return Err("there's no GitHub token to ask with".into());
                };
                github
                    .download_release_asset(asset)
                    .await
                    .map_err(|e| e.to_string())
            }
            Releases::Anonymous { .. } => {
                let bytes =
                    perch_github::releases::download(&asset.browser_download_url, asset.size)
                        .await
                        .map_err(|e| e.to_string())?;
                if bytes.len() as u64 != asset.size {
                    return Err(format!(
                        "{} came to {} bytes, not the {} its release lists",
                        asset.name,
                        bytes.len(),
                        asset.size
                    ));
                }
                Ok(bytes)
            }
        }
    }
}

/// The app's own engine looks for updates for as long as it runs. A
/// host's never does: the app on the Mac looks for its own.
pub(super) fn spawn(ctx: Ctx) {
    if ctx.host.is_some() {
        tracing::info!("an engine on a host updates no app, so not looking for updates");
        return;
    }
    tokio::spawn(clock(Releases::of(&ctx), ctx.events.clone()));
}

/// Check for Updates… in the engine: a host's answers that it can't, as
/// it did before apps looked for their own.
pub(super) async fn check_in(ctx: &Ctx) {
    if ctx.host.is_some() {
        let _ = ctx.events.send(Event::UpdateChecked(Checked::NotARelease));
        return;
    }
    check_now(&Releases::of(ctx), &ctx.events).await;
}

/// Restart to update in the engine: a host's refuses.
pub(super) async fn install_in(ctx: &Ctx, version: String) {
    if ctx.host.is_some() {
        let _ = ctx.events.send(Event::Update(UpdateStatus::Failed {
            version,
            error: "The engine on the host can't update this app.".into(),
        }));
        return;
    }
    install(&Releases::of(ctx), &ctx.events, version).await;
}

/// Look for updates from `releases` for as long as Perch runs, when it
/// runs from an app it can replace, telling `events`.
pub(crate) async fn clock(releases: Releases, events: EventSender<Event>) {
    if update::running_bundle().is_none() {
        tracing::info!("not running from a release, so not looking for updates");
        return;
    }
    tokio::time::sleep(FIRST_CHECK).await;
    let mut last: Option<SystemTime> = None;
    loop {
        let due = last.is_none_or(|at| at.elapsed().map_or(true, |gone| gone >= CHECK_EVERY));
        if due {
            last = Some(SystemTime::now());
            check(&releases, &events).await;
        }
        tokio::time::sleep(TICK).await;
    }
}

/// One look: a newer release comes down and `Ready` goes to every
/// window. A failed look is logged and waits for the next.
async fn check(releases: &Releases, events: &EventSender<Event>) {
    match fetch(releases, |_| {}).await {
        Ok(Some(ready)) => {
            let dir = update::updates_dir();
            let keep = ready.version.clone();
            let _ = tokio::task::spawn_blocking(move || update::prune(&dir, Some(&keep))).await;
            let _ = events.send(Event::Update(UpdateStatus::Ready {
                version: ready.version.to_string(),
            }));
        }
        Ok(None) => {
            let dir = update::updates_dir();
            let _ = tokio::task::spawn_blocking(move || update::prune(&dir, None)).await;
        }
        Err(e) => tracing::warn!("couldn't check for an update: {e}"),
    }
}

/// Check for Updates…: one look now, answered in the window that asked
/// whatever it finds. `Ready` still goes to every window.
pub(crate) async fn check_now(releases: &Releases, events: &EventSender<Event>) {
    let say = |found: Checked| {
        let _ = events.send(Event::UpdateChecked(found));
    };
    if update::running_bundle().is_none() {
        return say(Checked::NotARelease);
    }
    let downloading = |version: &Version| {
        say(Checked::Downloading {
            version: version.to_string(),
        })
    };
    match fetch(releases, downloading).await {
        Ok(Some(ready)) => {
            let version = ready.version.to_string();
            // The asking window first, so it says so once.
            say(Checked::Ready {
                version: version.clone(),
            });
            let _ = events.send(Event::Update(UpdateStatus::Ready { version }));
        }
        Ok(None) => say(Checked::Current),
        Err(e) => {
            tracing::warn!("couldn't check for an update: {e}");
            say(Checked::Failed(e));
        }
    }
}

/// One download at a time: the clock's look and one asked for unpack
/// into the same folder.
static FETCHING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A release newer than this build, unpacked.
struct Fetched {
    version: Version,
    app: PathBuf,
}

/// The latest release, downloaded and unpacked unless it already is.
/// None when this build is the latest. `downloading` hears the version
/// when there's a download to wait for.
async fn fetch(
    releases: &Releases,
    downloading: impl Fn(&Version),
) -> Result<Option<Fetched>, String> {
    let _one = FETCHING.lock().await;
    let Some(release) = releases.latest().await? else {
        return Ok(None);
    };
    let Some(version) = update::newer(&release.tag_name, &update::current_version()) else {
        return Ok(None);
    };
    let dir = update::updates_dir();
    if let Some(app) = update::staged(&dir, &version) {
        return Ok(Some(Fetched { version, app }));
    }
    let Some(asset) = release.asset(update::ASSET) else {
        tracing::info!("{} has no {}", release.tag_name, update::ASSET);
        return Ok(None);
    };
    tracing::info!("downloading Perch {version}");
    downloading(&version);
    let tarball = releases.download(asset).await?;
    let staging = version.clone();
    let app = tokio::task::spawn_blocking(move || update::stage(&dir, &staging, &tarball))
        .await
        .map_err(|e| e.to_string())??;
    tracing::info!("Perch {version} is ready at {}", app.display());
    Ok(Some(Fetched { version, app }))
}

/// Swap `version` in for the running app and open it once this process
/// has quit. `UpdateInstalled` tells the windows to quit; `Update(Failed)`
/// says why not, with the app as it was.
pub(crate) async fn install(releases: &Releases, events: &EventSender<Event>, version: String) {
    let failed = |error: String| {
        tracing::warn!("couldn't update to Perch {version}: {error}");
        let _ = events.send(Event::Update(UpdateStatus::Failed {
            version: version.clone(),
            error,
        }));
    };
    let Some(bundle) = update::running_bundle() else {
        failed("Perch isn't running from an app it can replace.".into());
        return;
    };
    // The download goes when an earlier try fails part way: fetch it again.
    let staged = match Version::parse(&version) {
        Ok(wanted) => match update::staged(&update::updates_dir(), &wanted) {
            Some(app) => Ok(app),
            None => match fetch(releases, |_| {}).await {
                Ok(Some(fetched)) if fetched.version == wanted => Ok(fetched.app),
                Ok(_) => Err(format!("Perch {version} is no longer the latest release.")),
                Err(e) => Err(e),
            },
        },
        Err(e) => Err(e.to_string()),
    };
    let staged = match staged {
        Ok(app) => app,
        Err(e) => return failed(e),
    };
    let swapped = tokio::task::spawn_blocking(move || {
        update::install(&staged, &bundle)?;
        update::relaunch_after_exit(&bundle)
            .map_err(|e| format!("The update is installed, but Perch couldn't reopen itself ({e}). Quit and open it again."))
    })
    .await;
    match swapped {
        Ok(Ok(())) => {
            tracing::info!("installed Perch {version}; quitting to reopen");
            let _ = events.send(Event::UpdateInstalled);
        }
        Ok(Err(e)) => failed(e),
        Err(e) => failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::tests::{ctx, nowhere, serve_by_method};

    fn failed_with(received: &std::sync::mpsc::Receiver<Event>) -> String {
        match received.try_iter().last() {
            Some(Event::Update(UpdateStatus::Failed { error, .. })) => error,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn the_apps_own_engine_asks_with_the_token_and_updates_its_app() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, received) = ctx(dir.path(), &nowhere(), &[]);
        assert!(ctx.host.is_none());
        *ctx.session.write().unwrap() = None;

        // Only the token's way of asking needs one.
        let asked = fetch(&Releases::of(&ctx), |_| {}).await;
        assert_eq!(asked.err().unwrap(), "there's no GitHub token to ask with");

        // A test runs from no app, so the engine says so, as ever.
        check_in(&ctx).await;
        assert!(matches!(
            received.try_iter().last(),
            Some(Event::UpdateChecked(Checked::NotARelease))
        ));
        install_in(&ctx, "9.9.9".into()).await;
        assert_eq!(
            failed_with(&received),
            "Perch isn't running from an app it can replace."
        );
    }

    #[tokio::test]
    async fn an_engine_on_a_host_updates_no_app() {
        let dir = tempfile::tempdir().unwrap();
        let (mut ctx, received) = ctx(dir.path(), &nowhere(), &[]);
        ctx.host = Some(Arc::default());

        check_in(&ctx).await;
        assert!(matches!(
            received.try_iter().last(),
            Some(Event::UpdateChecked(Checked::NotARelease))
        ));
        install_in(&ctx, "9.9.9".into()).await;
        assert_eq!(
            failed_with(&received),
            "The engine on the host can't update this app."
        );
    }

    #[tokio::test]
    async fn with_no_token_the_release_and_its_file_come_from_github_com() {
        let (base, paths) = serve_by_method(|_, path, headers| {
            assert!(
                !headers
                    .iter()
                    .any(|h| h.to_ascii_lowercase().starts_with("authorization:")),
                "{headers:?}"
            );
            match path {
                "/latest" => (
                    "200 OK",
                    r#"{"tag_name":"v9.9.9","html_url":"x","assets":[]}"#.into(),
                ),
                _ => ("200 OK", "perch".into()),
            }
        });
        let releases = Releases::Anonymous {
            latest: format!("{base}/latest"),
        };
        let release = releases.latest().await.unwrap().unwrap();
        assert_eq!(release.tag_name, "v9.9.9");

        let asset = |size| ReleaseAsset {
            id: 7,
            name: update::ASSET.into(),
            size,
            url: "https://api.github.com/repos/perch-code/perch/releases/assets/7".into(),
            browser_download_url: format!("{base}/download/v9.9.9/{}", update::ASSET),
        };
        assert_eq!(releases.download(&asset(5)).await.unwrap(), b"perch");
        // Short of what the release lists is no download.
        assert!(releases.download(&asset(6)).await.is_err());
        assert_eq!(
            paths.lock().unwrap()[..2],
            [
                "/latest".to_string(),
                format!("/download/v9.9.9/{}", update::ASSET)
            ]
        );
    }
}
