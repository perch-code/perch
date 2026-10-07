//! A pull request's Changescape, found in Changescape's cache off the UI
//! thread. File reads only: Perch computes nothing of a changescape.

use std::sync::Arc;

use perch_changescape::{Lookup, Page};

use super::{Ctx, Event};

/// What the cache holds for one pull request at one head, and its page
/// when there is one to draw.
#[derive(Debug, Clone)]
pub struct Found {
    pub lookup: Lookup,
    pub page: Option<Arc<Page>>,
    /// Why the page that should be there could not be read.
    pub error: Option<String>,
}

/// Look the pull request up and send what was found, `None` when
/// Changescape is turned off. Nothing is logged when there is no map, so a
/// reader without Changescape sees no trace of it.
pub(super) async fn lookup(
    ctx: &Ctx,
    item_id: String,
    repo: String,
    number: u64,
    head_sha: String,
) {
    let config = ctx.config();
    let found = if config.changescape.enabled {
        let root = config.changescape.cache_root();
        let head = head_sha.clone();
        tokio::task::spawn_blocking(move || find(&root, &repo, number, &head))
            .await
            .ok()
    } else {
        None
    };
    let _ = ctx.events.send(Event::Changescape {
        item_id,
        head_sha,
        found: found.map(Arc::new),
    });
}

pub(crate) fn find(root: &std::path::Path, repo: &str, number: u64, head_sha: &str) -> Found {
    let lookup = perch_changescape::lookup(root, repo, number, head_sha);
    let (page, error) = match lookup.drawable().map(perch_changescape::load) {
        Some(Ok(page)) => (Some(page), None),
        Some(Err(e)) => {
            tracing::warn!(error = %e, "could not read a changescape");
            (None, Some(e.to_string()))
        }
        None => (None, None),
    };
    Found {
        lookup,
        page,
        error,
    }
}
