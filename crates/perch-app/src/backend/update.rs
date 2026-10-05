//! Update's background half: ask GitHub for Perch's latest release now
//! and then, download and unpack it when it's newer, and swap it in when
//! the reader restarts.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use semver::Version;

use super::{Ctx, Event};
use crate::feedback::REPO;
use crate::update::{self, UpdateStatus};

/// How long after launch the first look waits: past the token, and past
/// the burst of reads a new window makes.
const FIRST_CHECK: Duration = Duration::from_secs(30);

/// How often to look. Releases are rare, and the look costs a 304.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// How often the clock is read. Tokio's timers stop while the Mac
/// sleeps, so six hours on one could be days of wall time; this wakes
/// often and compares against the wall clock.
const TICK: Duration = Duration::from_secs(15 * 60);

/// Look for updates for as long as Perch runs, when it runs from an app
/// it can replace.
pub(super) fn spawn(ctx: Ctx) {
    if update::running_bundle().is_none() {
        tracing::info!("not running from Perch.app, so not looking for updates");
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        let mut last: Option<SystemTime> = None;
        loop {
            let due = last.is_none_or(|at| at.elapsed().map_or(true, |gone| gone >= CHECK_EVERY));
            if due {
                last = Some(SystemTime::now());
                check(&ctx).await;
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

/// One look: a newer release comes down and `Ready` goes to every
/// window. A failed look is logged and waits for the next.
async fn check(ctx: &Ctx) {
    match fetch(ctx).await {
        Ok(Some(ready)) => {
            let dir = update::updates_dir();
            let keep = ready.version.clone();
            let _ = tokio::task::spawn_blocking(move || update::prune(&dir, Some(&keep))).await;
            let _ = ctx.events.send(Event::Update(UpdateStatus::Ready {
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

/// A release newer than this build, unpacked.
struct Fetched {
    version: Version,
    app: PathBuf,
}

/// The latest release, downloaded and unpacked unless it already is.
/// None when this build is the latest. No token is an error, not an
/// answer: it says nothing of what was downloaded before.
async fn fetch(ctx: &Ctx) -> Result<Option<Fetched>, String> {
    let Some(github) = ctx.github() else {
        return Err("there's no GitHub token to ask with".into());
    };
    let Some(release) = github
        .latest_release(REPO)
        .await
        .map_err(|e| e.to_string())?
    else {
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
    let tarball = github
        .download_release_asset(asset)
        .await
        .map_err(|e| e.to_string())?;
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
pub(super) async fn install(ctx: &Ctx, version: String) {
    let failed = |error: String| {
        tracing::warn!("couldn't update to Perch {version}: {error}");
        let _ = ctx.events.send(Event::Update(UpdateStatus::Failed {
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
            None => match fetch(ctx).await {
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
            let _ = ctx.events.send(Event::UpdateInstalled);
        }
        Ok(Err(e)) => failed(e),
        Err(e) => failed(e.to_string()),
    }
}
