//! Language servers kept running after their review is left.
//!
//! rust-analyzer takes a while to load a large workspace, and going back
//! to a pull request just left is common. So a server whose review closes,
//! or gives way to another's, is set aside rather than stopped, and an
//! open of the same worktree in the same window takes it up again. Only a
//! few are kept, and each only for a while: every one holds a whole
//! workspace in memory.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use super::WindowId;

/// How many servers are kept beside the ones on show.
const KEPT: usize = 2;

/// How long a server is kept unshown before it is stopped.
const KEEP_FOR: Duration = Duration::from_secs(10 * 60);

struct Kept {
    window: WindowId,
    client: Arc<perch_lsp::Client>,
    since: Instant,
}

/// The servers set aside, oldest first.
#[derive(Clone, Default)]
pub(super) struct KeptServers(Arc<Mutex<Vec<Kept>>>);

impl KeptServers {
    /// Set aside `client`, which `window` no longer shows. The files it was
    /// given are closed first, so it reads them from disk when taken up
    /// again. The oldest past `KEPT` is stopped, and this one is stopped
    /// after `KEEP_FOR` unless it is taken up before.
    pub(super) async fn keep(&self, window: WindowId, client: Arc<perch_lsp::Client>) {
        client.close_documents().await;
        let since = Instant::now();
        let overflow = {
            let Ok(mut kept) = self.0.lock() else {
                client.shutdown().await;
                return;
            };
            kept.push(Kept {
                window,
                client: client.clone(),
                since,
            });
            (kept.len() > KEPT).then(|| kept.remove(0).client)
        };
        if let Some(oldest) = overflow {
            tokio::spawn(async move { oldest.shutdown().await });
        }
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(KEEP_FOR).await;
            let expired = this.take_where(|k| k.since == since && Arc::ptr_eq(&k.client, &client));
            if let Some(client) = expired {
                client.shutdown().await;
            }
        });
    }

    /// The server `window` set aside for `root`, run by `binary`, taken
    /// back out.
    pub(super) fn take(
        &self,
        window: WindowId,
        root: &Path,
        binary: &Path,
    ) -> Option<Arc<perch_lsp::Client>> {
        self.take_where(|k| {
            k.window == window && k.client.root() == root && k.client.binary() == binary
        })
    }

    /// Stop every server `window` set aside.
    pub(super) async fn stop(&self, window: WindowId) {
        let stopping: Vec<_> = match self.0.lock() {
            Ok(mut kept) => {
                let (stopping, staying) = std::mem::take(&mut *kept)
                    .into_iter()
                    .partition(|k| k.window == window);
                *kept = staying;
                stopping
            }
            Err(_) => Vec::new(),
        };
        for kept in stopping {
            kept.client.shutdown().await;
        }
    }

    fn take_where(&self, wanted: impl Fn(&Kept) -> bool) -> Option<Arc<perch_lsp::Client>> {
        let mut kept = self.0.lock().ok()?;
        let i = kept.iter().position(wanted)?;
        Some(kept.remove(i).client)
    }
}
