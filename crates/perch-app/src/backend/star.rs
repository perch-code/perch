//! The one ask for a star on GitHub: whether it's due, and the star
//! itself when the reader says yes.

use super::{Ctx, Event};
use crate::feedback::REPO;

/// The meta key that says the ask has been made, or never needs to be.
const ASKED_KEY: &str = "star.asked";

/// The clock says the reader has been here a while. Ask, unless they
/// were asked before or have starred already. Recorded before the toast
/// goes up, so it is once whatever becomes of it. With GitHub out of
/// reach nothing is recorded, and the next launch looks again.
pub(super) async fn due(ctx: &Ctx) {
    let Some(Ok(None)) = ctx.with_cache(|c| c.meta(ASKED_KEY)) else {
        return;
    };
    let Some(github) = ctx.github() else {
        return;
    };
    let starred = match github.fetch_star(REPO).await {
        Ok((_, starred)) => starred,
        Err(e) => {
            tracing::debug!("couldn't read Perch's star: {e}");
            return;
        }
    };
    ctx.with_cache(|c| c.set_meta(ASKED_KEY, if starred { "starred" } else { "asked" }));
    if !starred {
        let _ = ctx.events.send(Event::StarAsk);
    }
}

/// Star Perch as the reader. `Starred` answers; one that couldn't be
/// added, by a token that may not star say, is left to the browser.
pub(super) async fn add(ctx: &Ctx) {
    let added = match ctx.github() {
        Some(github) => match github.fetch_star(REPO).await {
            Ok((_, true)) => Ok(()),
            Ok((id, false)) => github.add_star(&id).await,
            Err(e) => Err(e),
        },
        None => Err(perch_github::Error::Decode("not signed in".into())),
    };
    if let Err(e) = &added {
        tracing::warn!("couldn't star Perch: {e}");
    }
    let _ = ctx.events.send(Event::Starred {
        added: added.is_ok(),
    });
}
