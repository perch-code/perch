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
            .map(|kept| kept.iter().map(|k| k.client.clone()).collect())
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
    /// `binary`, taken back out.
    pub(super) fn take(
        &self,
        window: WindowId,
        root: &Path,
        kind: perch_lsp::ServerKind,
        binary: &Path,
    ) -> Option<Arc<perch_lsp::Client>> {
        self.take_where(|k| {
            k.window == window
                && k.client.root() == root
                && k.client.kind() == kind
                && k.client.binary() == binary
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
        let mut kept = self.0.lock().ok()?;
        let i = kept.iter().position(wanted)?;
        Some(kept.remove(i).client)
    }
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
}
