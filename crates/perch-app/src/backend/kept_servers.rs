//! Language servers kept running after their review is left.
//!
//! rust-analyzer takes a while to load a large workspace, and going back
//! to a pull request just left is common. So a server whose review closes,
//! or gives way to another's, is set aside rather than stopped, and an
//! open of the same worktree in the same window takes it up again. Only a
//! few are kept, and each only for a while: every one holds a whole
//! workspace in memory. Fewer still when every server together passes
//! `[lsp] max_memory_mb`: on a large workspace one kept server can cost as
//! much as the one on show.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use super::WindowId;

/// How many servers are kept beside the ones on show.
const KEPT: usize = 2;

/// How long a server is kept unshown before it is stopped.
const KEEP_FOR: Duration = Duration::from_secs(10 * 60);

/// How long the files a server was given have to close before it is set
/// aside. A server that stopped reading leaves the closes blocked on a
/// full pipe, and one kept that way would never be reached to be stopped.
const CLOSE_WAIT: Duration = Duration::from_secs(3);

struct Kept {
    window: WindowId,
    client: Arc<perch_lsp::Client>,
    since: Instant,
}

#[derive(Default)]
struct Shelf {
    /// Oldest first.
    kept: Vec<Kept>,
    /// The roots whose checkout went, and when (see `stop_root`). The
    /// clock is the one a server's age is told by, which tests don't pause.
    gone: Vec<(PathBuf, std::time::Instant)>,
}

impl Shelf {
    /// Whether `client` started before its root's checkout went: it
    /// watches that checkout, not what came to the path after.
    fn went_from_under(&self, client: &perch_lsp::Client) -> bool {
        self.gone
            .iter()
            .any(|(root, at)| root == client.root() && client.age() >= at.elapsed())
    }
}

/// The servers set aside.
#[derive(Clone, Default)]
pub(super) struct KeptServers(Arc<Mutex<Shelf>>);

impl KeptServers {
    /// Set aside `client`, which `window` no longer shows. The files it was
    /// given are closed first, so it reads them from disk when taken up
    /// again. The oldest past `KEPT` is stopped, and this one is stopped
    /// after `KEEP_FOR` unless it is taken up before. One whose files
    /// can't be closed within `CLOSE_WAIT` is stopped instead of kept, one
    /// that has ended is let go, and one whose checkout went meanwhile is
    /// stopped now.
    pub(super) async fn keep(&self, window: WindowId, client: Arc<perch_lsp::Client>) {
        if client.has_ended() {
            tracing::info!(
                server = client.kind().name(),
                "language server let go, not kept: it has ended"
            );
            let_go(client);
            return;
        }
        if tokio::time::timeout(CLOSE_WAIT, client.close_documents())
            .await
            .is_err()
        {
            tracing::warn!(
                server = client.kind().name(),
                "language server stopped, not kept: its files could not be closed"
            );
            tokio::spawn(async move { client.shutdown().await });
            return;
        }
        let since = Instant::now();
        // Checked under the lock `stop_root` marks the root under, so a
        // set-aside lands before the stop, which stops it, or after, which
        // sees the checkout went.
        let kept = self.0.lock().ok().and_then(|mut shelf| {
            if shelf.went_from_under(&client) {
                tracing::info!(
                    server = client.kind().name(),
                    "language server stopped: its checkout went"
                );
                return None;
            }
            shelf.kept.push(Kept {
                window,
                client: client.clone(),
                since,
            });
            Some((shelf.kept.len() > KEPT).then(|| shelf.kept.remove(0).client))
        });
        let Some(overflow) = kept else {
            client.shutdown().await;
            return;
        };
        if let Some(oldest) = overflow {
            tracing::info!(
                server = oldest.kind().name(),
                "kept language server stopped: too many kept"
            );
            tokio::spawn(async move { oldest.shutdown().await });
        }
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(KEEP_FOR).await;
            let expired = this.take_where(|k| k.since == since && Arc::ptr_eq(&k.client, &client));
            if let Some(client) = expired {
                tracing::info!(
                    server = client.kind().name(),
                    "kept language server stopped: kept too long"
                );
                client.shutdown().await;
            }
        });
    }

    /// Every server set aside, for a bug report.
    pub(super) fn clients(&self) -> Vec<Arc<perch_lsp::Client>> {
        self.0
            .lock()
            .map(|shelf| shelf.kept.iter().map(|k| k.client.clone()).collect())
            .unwrap_or_default()
    }

    /// Set aside every server of a worktree `window` no longer shows.
    pub(super) async fn keep_all(
        &self,
        window: WindowId,
        clients: std::collections::HashMap<perch_lsp::ServerKind, Arc<perch_lsp::Client>>,
    ) {
        for client in clients.into_values() {
            self.keep(window, client).await;
        }
    }

    /// The server of `kind` that `window` set aside for `root`, run by
    /// `binary`, taken back out. Those that have ended since they were set
    /// aside are let go first: taken up, a crashed one would stand in the
    /// way of a new one.
    pub(super) fn take(
        &self,
        window: WindowId,
        root: &Path,
        kind: perch_lsp::ServerKind,
        binary: &Path,
    ) -> Option<Arc<perch_lsp::Client>> {
        let ended: Vec<_> = match self.0.lock() {
            Ok(mut shelf) => {
                let (ended, running) = std::mem::take(&mut shelf.kept)
                    .into_iter()
                    .partition(|k: &Kept| k.client.has_ended());
                shelf.kept = running;
                ended
            }
            Err(_) => Vec::new(),
        };
        for kept in ended {
            tracing::info!(
                server = kept.client.kind().name(),
                "kept language server let go: it has ended"
            );
            let_go(kept.client);
        }
        self.take_where(|k| {
            k.window == window
                && k.client.root() == root
                && k.client.kind() == kind
                && k.client.binary() == binary
        })
    }

    /// Stop every server `window` set aside.
    pub(super) async fn stop(&self, window: WindowId) {
        self.stop_where(|k| k.window == window).await;
    }

    /// Stop every server set aside for `root`, in any window: its checkout
    /// is going, and what comes to the same path next may be other code,
    /// which a server still watching the path would load. One started
    /// before now and set aside after, as a start or a set-aside still
    /// under way would be, is stopped as it comes (see `keep`).
    pub(super) async fn stop_root(&self, root: &Path) {
        if let Ok(mut shelf) = self.0.lock() {
            // Long past anything still under way.
            shelf.gone.retain(|(_, at)| at.elapsed() < KEEP_FOR);
            shelf
                .gone
                .push((root.to_path_buf(), std::time::Instant::now()));
        }
        self.stop_where(|k| k.client.root() == root).await;
    }

    /// Stop every server of `kind` set aside for `root`, in any window:
    /// the code there isn't trusted now, and set aside it would go on
    /// loading it.
    pub(super) async fn stop_kind_at(&self, root: &Path, kind: perch_lsp::ServerKind) {
        self.stop_where(|k| k.client.root() == root && k.client.kind() == kind)
            .await;
    }

    async fn stop_where(&self, wanted: impl Fn(&Kept) -> bool) {
        let stopping: Vec<_> = match self.0.lock() {
            Ok(mut shelf) => {
                let (stopping, staying) = std::mem::take(&mut shelf.kept)
                    .into_iter()
                    .partition(wanted);
                shelf.kept = staying;
                stopping
            }
            Err(_) => Vec::new(),
        };
        for kept in stopping {
            kept.client.shutdown().await;
        }
    }

    /// Stop kept servers, oldest first, while every server Perch runs uses
    /// more than `budget_kb` between them. `active` are the ones on show:
    /// they count toward the total but are never stopped. A server's
    /// memory is its process and every process under it, rust-analyzer's
    /// proc-macro server among them.
    pub(super) async fn trim(&self, budget_kb: u64, active: &[Arc<perch_lsp::Client>]) {
        let kept = self.clients();
        if kept.is_empty() {
            return;
        }
        let pids: Vec<u32> = active.iter().chain(&kept).filter_map(|c| c.pid()).collect();
        let memory = process_tree_memory(&pids).await;
        let size = |client: &perch_lsp::Client| {
            client
                .pid()
                .and_then(|pid| memory.get(&pid).copied())
                .unwrap_or(0)
        };
        let mut total: u64 = active.iter().chain(&kept).map(|c| size(c)).sum();
        for client in kept {
            if total <= budget_kb {
                break;
            }
            // Taken up again since `clients`: on show now, so it stays.
            if self
                .take_where(|k| Arc::ptr_eq(&k.client, &client))
                .is_none()
            {
                continue;
            }
            let freed = size(&client);
            tracing::info!(
                server = client.kind().name(),
                freed_mb = freed / 1024,
                total_mb = total / 1024,
                budget_mb = budget_kb / 1024,
                "kept language server stopped: over the memory budget"
            );
            total = total.saturating_sub(freed);
            client.shutdown().await;
        }
    }

    fn take_where(&self, wanted: impl Fn(&Kept) -> bool) -> Option<Arc<perch_lsp::Client>> {
        let mut shelf = self.0.lock().ok()?;
        let i = shelf.kept.iter().position(wanted)?;
        Some(shelf.kept.remove(i).client)
    }
}

/// Let go of a server that has ended. Stopped all the same, which is quick
/// for one that's gone: a failure it has yet to tell would read as that of
/// the one that takes its place, and one whose output closed while it ran
/// on is killed.
pub(super) fn let_go(client: Arc<perch_lsp::Client>) {
    tokio::spawn(async move { client.shutdown().await });
}

/// The resident memory in KB of each of `pids`, with that of every process
/// under it added in, by pid. A pid `ps` doesn't list is left out; empty
/// when `ps` can't be run.
async fn process_tree_memory(pids: &[u32]) -> HashMap<u32, u64> {
    let mut command = tokio::process::Command::new("ps");
    command.args(["-axo", "pid=,ppid=,rss="]).kill_on_drop(true);
    let out = match tokio::time::timeout(Duration::from_secs(5), command.output()).await {
        Ok(Ok(out)) if out.status.success() => out.stdout,
        _ => return HashMap::new(),
    };
    tree_memory(&String::from_utf8_lossy(&out), pids)
}

/// `ps -o pid=,ppid=,rss=` output as `process_tree_memory` gives it, for
/// `pids` alone: a walk from every process would be one of the whole
/// machine from pid 1.
fn tree_memory(ps: &str, pids: &[u32]) -> HashMap<u32, u64> {
    let mut own = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in ps.lines() {
        let mut parts = line.split_whitespace().map(str::parse::<u64>);
        let (Some(Ok(pid)), Some(Ok(ppid)), Some(Ok(rss))) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (pid, ppid) = (pid as u32, ppid as u32);
        own.insert(pid, rss);
        if ppid != pid {
            children.entry(ppid).or_default().push(pid);
        }
    }
    pids.iter()
        .filter(|pid| own.contains_key(pid))
        .map(|&pid| {
            let mut total = 0;
            let mut stack = vec![pid];
            let mut seen = std::collections::HashSet::new();
            while let Some(next) = stack.pop() {
                if !seen.insert(next) {
                    continue;
                }
                total += own.get(&next).copied().unwrap_or(0);
                stack.extend(children.get(&next).into_iter().flatten());
            }
            (pid, total)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_counts_everything_under_it() {
        let ps = "    1     0   100\n   10     1  5000\n   11    10   300\n   12    11    20\n   20     1   700\n";
        let memory = tree_memory(ps, &[10, 11, 20, 1]);
        assert_eq!(memory[&10], 5320);
        assert_eq!(memory[&11], 320);
        assert_eq!(memory[&20], 700);
        assert_eq!(memory[&1], 6120);
    }

    #[test]
    fn lines_that_are_not_three_numbers_are_skipped() {
        let memory = tree_memory("garbage\n  5  1  40\n  6  1\n", &[5, 6]);
        assert_eq!(memory.len(), 1);
        assert_eq!(memory[&5], 40);
    }

    #[test]
    fn only_the_processes_asked_for_are_summed() {
        let ps = "    1     0   100\n   10     1  5000\n   11    10   300\n   20     1   700\n";
        let memory = tree_memory(ps, &[10, 30]);
        assert_eq!(memory.len(), 1, "not pid 1, nor one ps doesn't list");
        assert_eq!(memory[&10], 5300);
    }

    /// A server that answers the handshake and then reads nothing more.
    const DEAF_SERVER: &str = r#"
import json, sys, time
length = None
while True:
    line = sys.stdin.buffer.readline()
    if not line:
        sys.exit(0)
    if line in (b"\r\n", b"\n"):
        break
    name, _, value = line.decode().partition(":")
    if name.lower() == "content-length":
        length = int(value)
message = json.loads(sys.stdin.buffer.read(length))
body = json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {}}}).encode()
sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
sys.stdout.buffer.flush()
time.sleep(600)
"#;

    /// Whether a process is running, as `kill -0` finds it.
    fn alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// A server that stopped reading can't take its files' closes: it is
    /// stopped rather than kept, and setting it aside doesn't wait on it
    /// for good.
    #[tokio::test]
    async fn a_server_that_cannot_take_its_closes_is_stopped_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(&script, DEAF_SERVER).unwrap();
        let spec = perch_lsp::ServerSpec {
            kind: perch_lsp::ServerKind::RustAnalyzer,
            binary: "python3".into(),
            args: vec![script.display().to_string()],
            options: serde_json::Value::Null,
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let client = perch_lsp::Client::start(spec, dir.path(), tx).await.unwrap();
        let pid = client.pid().unwrap();

        // A file too big for the pipe: its open blocks holding the
        // server's input, and every close after it waits.
        let path = dir.path().join("lib.rs");
        let (opening, opened) = (client.clone(), path.clone());
        tokio::spawn(async move { opening.did_open(&opened, &"x".repeat(1 << 20)).await });
        while client.given(&path).is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let kept = KeptServers::default();
        let keeping = kept.keep(1, client);
        let waited = tokio::time::timeout(CLOSE_WAIT + Duration::from_secs(2), keeping).await;
        assert!(waited.is_ok(), "set aside for good");
        assert!(kept.clients().is_empty(), "kept");
        let gone = async {
            while alive(pid) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        let gone = tokio::time::timeout(Duration::from_secs(15), gone).await;
        assert!(gone.is_ok(), "still running");
    }

    #[tokio::test]
    async fn a_removed_worktree_s_kept_servers_stop_in_every_window() {
        use crate::backend::tests::named_server;
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let rust = perch_lsp::ServerKind::RustAnalyzer;
        let on_a = named_server(a.path(), rust, "-").await;
        let on_b = named_server(b.path(), rust, "-").await;
        let kept = KeptServers::default();
        kept.keep(1, on_a).await;
        kept.keep(2, on_b.clone()).await;

        kept.stop_root(a.path()).await;
        let left = kept.clients();
        assert_eq!(left.len(), 1);
        assert!(Arc::ptr_eq(&left[0], &on_b), "another worktree's stays");
        kept.stop_root(b.path()).await;
        assert!(kept.clients().is_empty());
    }

    /// A server set aside after its checkout went, as one still starting
    /// or still closing its files would be, is stopped as it comes. One
    /// started at the path since, for what came there next, is kept.
    #[tokio::test]
    async fn a_server_set_aside_after_its_checkout_went_is_stopped() {
        use crate::backend::tests::named_server;
        let tree = tempfile::tempdir().unwrap();
        let rust = perch_lsp::ServerKind::RustAnalyzer;
        let late = named_server(tree.path(), rust, "-").await;
        let kept = KeptServers::default();

        kept.stop_root(tree.path()).await;
        kept.keep(1, late).await;
        assert!(
            kept.clients().is_empty(),
            "it watched the checkout that went"
        );

        let next = named_server(tree.path(), rust, "-").await;
        kept.keep(1, next.clone()).await;
        let left = kept.clients();
        assert_eq!(left.len(), 1);
        assert!(Arc::ptr_eq(&left[0], &next), "the code that came next");
        next.shutdown().await;
    }
}
