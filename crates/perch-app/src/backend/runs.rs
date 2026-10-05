//! Admission, lifetime, and cancellation for every agent Perch launches.
//!
//! A run is durable before it enters the fair semaphore queue.  The live
//! entry is deliberately small: durable history belongs to `perch-github`,
//! while this module owns only the process and the right to execute it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use perch_github::{
    NewRunEvent, RunEvent, RunMode, RunOwner, RunRecord, RunStatus, TerminalReason,
};
use tokio::sync::{watch, Notify, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

use super::{Event, SharedCache};

#[derive(Clone)]
pub(crate) struct RunManager {
    inner: Arc<Inner>,
}

struct Inner {
    limit: Arc<Semaphore>,
    /// One Work run at a time in a worktree, in the order they came. A
    /// Work run takes its worktree's lock before a slot, so one waiting
    /// on another Work run does not hold a slot from anyone else.
    worktrees: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    admission: Mutex<AdmissionGate>,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    cache: SharedCache,
    events: super::EventSender<Event>,
}

#[derive(Default)]
struct AdmissionGate {
    closed: bool,
    blocked: HashMap<OwnerFence, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum OwnerFence {
    Item(String),
    Discussion { item_id: String, thread_id: String },
}

struct Entry {
    id: String,
    owner: RunOwner,
    /// For a Work run, the worktree it changes, as the disk resolves it.
    worktree: Option<PathBuf>,
    cancelled: AtomicBool,
    /// The task which owned the spawn lease has finished. If its terminal
    /// database write failed, keep the entry registered and retry that
    /// projection instead of leaving an unowned durable active row.
    execution_done: AtomicBool,
    finished: AtomicBool,
    cancel: Notify,
    done: watch::Sender<bool>,
    process: Mutex<Option<perch_agent::Run>>,
    /// What the reader said before the process came up, for `attach` to
    /// give it. Guarded by `process`'s lock, which is taken first.
    steers: Mutex<Vec<String>>,
    stop_reason: Mutex<TerminalReason>,
    pending_terminal: Mutex<Option<PendingTerminal>>,
}

#[derive(Clone)]
struct PendingTerminal {
    status: RunStatus,
    reason: TerminalReason,
    message: String,
}

/// A request waiting for the shared execution limit.
pub(crate) struct Admission {
    entry: Arc<Entry>,
}

/// Keeps one scheduler slot occupied until the backend has consumed the
/// process's terminal event and committed it.
pub(crate) struct RunLease {
    entry: Arc<Entry>,
    _permit: OwnedSemaphorePermit,
    _worktree: Option<OwnedMutexGuard<()>>,
}

/// Prevents a new run for an owner while its durable records are being
/// deleted. The count makes duplicate delete commands safe.
pub(crate) struct OwnerDeletion {
    manager: RunManager,
    fence: OwnerFence,
}

impl RunManager {
    pub(crate) fn new(limit: usize, cache: SharedCache, events: super::EventSender<Event>) -> Self {
        Self {
            inner: Arc::new(Inner {
                limit: Arc::new(Semaphore::new(limit.max(1))),
                worktrees: Mutex::new(HashMap::new()),
                admission: Mutex::new(AdmissionGate::default()),
                entries: Mutex::new(HashMap::new()),
                cache,
                events,
            }),
        }
    }

    /// Persist a queued run and expose it to the app before returning.  A
    /// discussion may have only one active answer, including one waiting in
    /// the queue.
    pub(crate) fn admit(&self, run: RunRecord) -> Result<Admission, String> {
        if run.status != RunStatus::Queued {
            return Err("a new agent run must enter the queue first".into());
        }
        // Deletion takes this gate before it snapshots live entries. Holding
        // it through durable insertion and live registration means a run is
        // either rejected or necessarily included in that snapshot.
        let gate = self
            .inner
            .admission
            .lock()
            .map_err(|_| "The agent scheduler is unavailable")?;
        if gate.closed {
            return Err("Perch is shutting down; no new agent runs can start".into());
        }
        if owner_is_blocked(&gate, &run.owner) {
            return Err("That discussion or item is being deleted".into());
        }
        let mut entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| "The agent scheduler is unavailable")?;
        let cache = self
            .inner
            .cache
            .as_ref()
            .ok_or("The local database is unavailable")?;
        let guard = cache.lock().map_err(|_| "The local database is busy")?;
        if !owner_exists(&guard, &run.owner)? {
            return Err("The discussion or item for that run is gone".into());
        }
        let active = guard.active_run_summaries().map_err(|e| e.to_string())?;
        if let RunOwner::Discussion { thread_id, .. } = &run.owner {
            if active.iter().any(|other| {
                other.id != run.id
                    && matches!(&other.owner, RunOwner::Discussion { thread_id: other_thread, .. } if other_thread == thread_id)
            }) {
                return Err("That discussion already has an active answer".into());
            }
        }
        let queued = NewRunEvent {
            event_id: "state-queued".into(),
            occurred_at: run.queued_at,
            event: RunEvent::State {
                status: RunStatus::Queued,
            },
            truncated: false,
        };
        // Do not perform a fallible read between the durable insert and live
        // registration. Once the transaction commits, this entry must remain
        // reachable by cancellation and shutdown even if later reads fail.
        let run = if let Err(insert_error) = guard.insert_run_with_event(&run, &queued) {
            // SQLite can report an error while committing. Reconcile the
            // ambiguous result before returning so a committed queued row is
            // never left without a scheduler owner.
            match guard.load_run(&run.id) {
                Ok(Some(saved))
                    if !entries.contains_key(&run.id)
                        && saved.status == RunStatus::Queued
                        && saved.owner == run.owner
                        && saved.purpose == run.purpose
                        && saved.original_request == run.original_request =>
                {
                    saved
                }
                _ => return Err(insert_error.to_string()),
            }
        } else {
            run
        };
        let (done, _) = watch::channel(false);
        let worktree = (run.mode == RunMode::Work)
            .then(|| run.working_directory.as_deref().map(PathBuf::from))
            .flatten()
            .map(|path| path.canonicalize().unwrap_or(path));
        let entry = Arc::new(Entry {
            id: run.id.clone(),
            owner: run.owner.clone(),
            worktree,
            cancelled: AtomicBool::new(false),
            execution_done: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            cancel: Notify::new(),
            done,
            process: Mutex::new(None),
            steers: Mutex::new(Vec::new()),
            stop_reason: Mutex::new(TerminalReason::UserStopped),
            pending_terminal: Mutex::new(None),
        });
        entries.insert(run.id.clone(), entry.clone());
        drop(guard);
        drop(entries);
        drop(gate);
        let _ = self.inner.events.send(Event::AgentRunUpdated { run });
        Ok(Admission { entry })
    }

    /// Fence an item or one discussion against admission until the returned
    /// guard is dropped. Call this before cancelling its current runs and
    /// hold it until the database deletion has committed or failed.
    pub(crate) fn block_owner(
        &self,
        item_id: &str,
        thread_id: Option<&str>,
    ) -> Result<OwnerDeletion, String> {
        let fence = match thread_id {
            Some(thread_id) => OwnerFence::Discussion {
                item_id: item_id.to_string(),
                thread_id: thread_id.to_string(),
            },
            None => OwnerFence::Item(item_id.to_string()),
        };
        let mut gate = self
            .inner
            .admission
            .lock()
            .map_err(|_| "The agent scheduler is unavailable")?;
        *gate.blocked.entry(fence.clone()).or_default() += 1;
        drop(gate);
        Ok(OwnerDeletion {
            manager: self.clone(),
            fence,
        })
    }

    /// Wait in FIFO order for an execution slot. Cancellation wins without
    /// spawning, even when it races with a newly available permit.
    pub(crate) async fn start(&self, admission: Admission) -> Option<RunLease> {
        let entry = admission.entry;
        if entry.cancelled.load(Ordering::Acquire) {
            self.finish_cancelled_queue(&entry);
            return None;
        }
        let worktree = match &entry.worktree {
            None => None,
            Some(path) => {
                let lock = self.worktree_lock(path);
                match lock.clone().try_lock_owned() {
                    Ok(guard) => Some(guard),
                    Err(_) => {
                        self.set_activity(&entry.id, "Waiting for the Work run in this worktree");
                        let wait = lock.lock_owned();
                        tokio::pin!(wait);
                        let guard = tokio::select! {
                            biased;
                            _ = entry.cancel.notified() => {
                                self.finish_cancelled_queue(&entry);
                                return None;
                            }
                            guard = &mut wait => guard,
                        };
                        if entry.cancelled.load(Ordering::Acquire) {
                            self.finish_cancelled_queue(&entry);
                            return None;
                        }
                        self.set_activity(&entry.id, "Waiting for an execution slot");
                        Some(guard)
                    }
                }
            }
        };
        let acquire = self.inner.limit.clone().acquire_owned();
        tokio::pin!(acquire);
        let permit = tokio::select! {
            biased;
            _ = entry.cancel.notified() => {
                self.finish_cancelled_queue(&entry);
                return None;
            }
            permit = &mut acquire => match permit {
                Ok(permit) => permit,
                Err(_) => {
                    self.complete_entry(
                        &entry,
                        RunStatus::Interrupted,
                        TerminalReason::AppShutdown,
                        "Perch stopped before the run could start".into(),
                    );
                    return None;
                }
            },
        };
        if entry.cancelled.load(Ordering::Acquire) {
            self.finish_cancelled_queue(&entry);
            return None;
        }
        let started = self.transition(&entry.id, RunStatus::Starting, None, None);
        if !started
            .as_ref()
            .is_some_and(|run| run.status == RunStatus::Starting)
        {
            // A queued cancellation can commit while the semaphore wakes.
            // Never hand out a spawn lease unless Starting itself is the
            // durable state we just observed.
            if started.as_ref().is_some_and(|run| run.status.is_terminal()) {
                self.finish_entry(&entry);
            } else if entry.cancelled.load(Ordering::Acquire) {
                self.finish_cancelled_queue(&entry);
            } else {
                self.complete_entry(
                    &entry,
                    RunStatus::Failed,
                    TerminalReason::Other,
                    "Could not save the agent's starting state".into(),
                );
            }
            return None;
        }
        Some(RunLease {
            entry,
            _permit: permit,
            _worktree: worktree,
        })
    }

    /// The lock Work runs in `path` take turns on.
    fn worktree_lock(&self, path: &std::path::Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .inner
            .worktrees
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.entry(path.to_path_buf()).or_default().clone()
    }

    /// Say what a queued run is waiting for, in its row.
    fn set_activity(&self, id: &str, activity: &str) {
        let Some(cache) = self.inner.cache.as_ref() else {
            return;
        };
        let Ok(guard) = cache.lock() else {
            return;
        };
        let Some(mut run) = guard.load_run(id).ok().flatten() else {
            return;
        };
        if run.status != RunStatus::Queued {
            return;
        }
        run.latest_activity = Some(activity.to_string());
        if guard.update_run(&run).is_ok() {
            drop(guard);
            let _ = self.inner.events.send(Event::AgentRunUpdated { run });
        }
    }

    /// Attach the process before publishing Running. A stop that arrived in
    /// the Starting window is immediately forwarded to the new process.
    pub(crate) fn attach(&self, lease: &RunLease, process: perch_agent::Run) {
        if let Ok(mut slot) = lease.entry.process.lock() {
            *slot = Some(process.clone());
            if let Ok(mut steers) = lease.entry.steers.lock() {
                for text in steers.drain(..) {
                    process.steer(&text);
                }
            }
        }
        if lease.entry.cancelled.load(Ordering::Acquire) {
            self.transition(&lease.entry.id, RunStatus::Stopping, None, None);
            process.cancel();
        } else {
            self.transition(&lease.entry.id, RunStatus::Running, None, None);
        }
    }

    /// Give a live run the reader's message: now, or as its process comes
    /// up. One the run never reads is its caller's to deal with.
    pub(crate) fn steer(&self, id: &str, text: &str) {
        let entry = self
            .inner
            .entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(id).cloned());
        let Some(entry) = entry else { return };
        let Ok(slot) = entry.process.lock() else {
            return;
        };
        match slot.as_ref() {
            Some(process) => {
                process.steer(text);
            }
            None => {
                if let Ok(mut steers) = entry.steers.lock() {
                    steers.push(text.to_string());
                }
            }
        }
    }

    /// Stop or cancel one live run. Terminal history is immutable.
    pub(crate) fn cancel(&self, id: &str) {
        self.cancel_with_reason(id, TerminalReason::UserStopped);
    }

    fn cancel_with_reason(&self, id: &str, reason: TerminalReason) {
        let entry = self
            .inner
            .entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(id).cloned());
        let Some(entry) = entry else { return };
        if let Ok(mut stored) = entry.stop_reason.lock() {
            *stored = reason;
        }
        entry.cancelled.store(true, Ordering::Release);
        if entry.execution_done.load(Ordering::Acquire) {
            self.reconcile_entry(&entry);
            return;
        }
        let process = entry.process.lock().ok().and_then(|slot| slot.clone());
        if let Some(process) = process {
            self.transition(id, RunStatus::Stopping, None, None);
            process.cancel();
        } else {
            // Wake both a queued waiter and the narrow Starting window. The
            // former commits Stopped; the latter attaches and forwards stop.
            // `notify_one` retains a permit if the fair-semaphore waiter is
            // between its atomic check and polling `notified()`.
            entry.cancel.notify_one();
            let status = self.load(id).map(|r| r.status);
            if matches!(status, Some(RunStatus::Starting)) {
                self.transition(id, RunStatus::Stopping, None, None);
            } else if matches!(status, Some(RunStatus::Queued)) {
                self.finish_cancelled_queue(&entry);
            }
        }
    }

    pub(crate) fn stop_terminal(&self, id: &str) -> (RunStatus, TerminalReason) {
        let reason = self
            .inner
            .entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(id).cloned())
            .and_then(|entry| entry.stop_reason.lock().ok().map(|reason| *reason))
            .unwrap_or(TerminalReason::UserStopped);
        if reason == TerminalReason::AppShutdown {
            (RunStatus::Interrupted, reason)
        } else {
            (RunStatus::Stopped, reason)
        }
    }

    pub(crate) fn finish(&self, lease: RunLease) {
        let entry = lease.entry.clone();
        let (status, reason, message) = if entry.cancelled.load(Ordering::Acquire) {
            let (status, reason) = self.stop_terminal(&entry.id);
            let message = if reason == TerminalReason::AppShutdown {
                "Perch shut down while the agent was running"
            } else {
                "Stopped by the reader"
            };
            (status, reason, message.to_string())
        } else {
            (
                RunStatus::Failed,
                TerminalReason::Other,
                "The agent exited, but Perch could not save its final state".into(),
            )
        };
        self.complete_entry(&entry, status, reason, message);
        drop(lease);
    }

    /// Cancel and await every live run owned by an item or discussion.
    pub(crate) async fn cancel_owner(&self, item_id: &str, thread_id: Option<&str>) {
        let entries: Vec<_> = self
            .inner
            .entries
            .lock()
            .map(|entries| {
                entries
                    .values()
                    .filter(|entry| owner_matches(&entry.owner, item_id, thread_id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for entry in &entries {
            self.cancel_with_reason(&entry.id, TerminalReason::UserStopped);
        }
        for entry in entries {
            self.wait_entry_done(&entry).await;
        }
    }

    /// Whether a Work run is queued or running in `worktree`: it may
    /// commit at any moment, which a push should not race.
    pub(crate) fn has_active_work(&self, worktree: &Path) -> bool {
        let worktree = worktree
            .canonicalize()
            .unwrap_or_else(|_| worktree.to_path_buf());
        self.inner.entries.lock().is_ok_and(|entries| {
            entries.values().any(|entry| {
                entry.worktree.as_deref() == Some(worktree.as_path())
                    && !entry.finished.load(Ordering::SeqCst)
            })
        })
    }

    pub(crate) fn has_active_owner(&self, item_id: &str, thread_id: Option<&str>) -> bool {
        self.inner.entries.lock().is_ok_and(|entries| {
            entries
                .values()
                .any(|entry| owner_matches(&entry.owner, item_id, thread_id))
        })
    }

    /// Wait for all work already owned by an item or discussion to finish
    /// without cancelling it. Callers that persist the item as archived
    /// first also prevent later admission through `owner_exists`.
    pub(crate) async fn wait_owner_idle(&self, item_id: &str, thread_id: Option<&str>) {
        loop {
            let entries: Vec<_> = self
                .inner
                .entries
                .lock()
                .map(|entries| {
                    entries
                        .values()
                        .filter(|entry| owner_matches(&entry.owner, item_id, thread_id))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if entries.is_empty() {
                return;
            }
            for entry in entries {
                self.wait_entry_done(&entry).await;
            }
        }
    }

    /// Normal shutdown owns and reaps its children. The process layer has
    /// its own TERM/KILL bound; this outer bound prevents a broken adapter
    /// from keeping the application alive forever.
    pub(crate) async fn shutdown(&self) {
        // `admit` holds this same gate through durable insertion and live
        // registration. Once this lock closes admission, every request which
        // won the race is necessarily visible in `entries`, and every later
        // request is rejected before it can spend money or touch the queue.
        {
            let mut gate = self
                .inner
                .admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            gate.closed = true;
            self.inner.limit.close();
        }
        let entries: Vec<_> = self
            .inner
            .entries
            .lock()
            .map(|entries| entries.values().cloned().collect())
            .unwrap_or_default();
        for entry in &entries {
            self.cancel_with_reason(&entry.id, TerminalReason::AppShutdown);
        }
        self.reconcile_finished();
        let wait = async {
            for entry in entries {
                self.wait_entry_done(&entry).await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(8), wait).await;
    }

    fn finish_cancelled_queue(&self, entry: &Arc<Entry>) {
        let reason = entry
            .stop_reason
            .lock()
            .map(|reason| *reason)
            .unwrap_or(TerminalReason::QueueCancelled);
        let (status, reason, message) = if reason == TerminalReason::AppShutdown {
            (
                RunStatus::Interrupted,
                TerminalReason::AppShutdown,
                "Perch shut down before the run started",
            )
        } else {
            (
                RunStatus::Stopped,
                TerminalReason::QueueCancelled,
                "Cancelled before starting",
            )
        };
        self.complete_entry(entry, status, reason, message.into());
    }

    /// Retry terminal projections for tasks whose process is already gone.
    /// These entries intentionally remain live if SQLite could not accept a
    /// terminal write: cancellation/deletion/shutdown can still find them,
    /// and startup remains a final interruption fence after a hard failure.
    pub(crate) fn reconcile_finished(&self) {
        let entries: Vec<_> = self
            .inner
            .entries
            .lock()
            .map(|entries| {
                entries
                    .values()
                    .filter(|entry| entry.execution_done.load(Ordering::Acquire))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for entry in entries {
            self.reconcile_entry(&entry);
        }
    }

    fn complete_entry(
        &self,
        entry: &Arc<Entry>,
        status: RunStatus,
        reason: TerminalReason,
        message: String,
    ) {
        entry.execution_done.store(true, Ordering::Release);
        if let Ok(mut process) = entry.process.lock() {
            *process = None;
        }
        if let Ok(mut pending) = entry.pending_terminal.lock() {
            pending.get_or_insert(PendingTerminal {
                status,
                reason,
                message,
            });
        }
        self.reconcile_entry(entry);
    }

    fn reconcile_entry(&self, entry: &Arc<Entry>) {
        if self
            .load(&entry.id)
            .is_some_and(|run| run.status.is_terminal())
        {
            self.finish_entry(entry);
            return;
        }
        let pending = entry
            .pending_terminal
            .lock()
            .ok()
            .and_then(|pending| pending.clone());
        let Some(pending) = pending else { return };
        if self
            .transition(
                &entry.id,
                pending.status,
                Some(pending.reason),
                Some(pending.message),
            )
            .is_some_and(|run| run.status.is_terminal())
        {
            self.finish_entry(entry);
        }
    }

    async fn wait_entry_done(&self, entry: &Arc<Entry>) {
        loop {
            if entry.execution_done.load(Ordering::Acquire) {
                self.reconcile_entry(entry);
            }
            if entry.finished.load(Ordering::Acquire) {
                return;
            }
            tokio::select! {
                _ = entry.wait_done() => return,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    }

    fn finish_entry(&self, entry: &Arc<Entry>) {
        if entry.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut entries) = self.inner.entries.lock() {
            entries.remove(&entry.id);
        }
        if let Ok(mut process) = entry.process.lock() {
            *process = None;
        }
        entry.done.send_replace(true);
    }

    pub(crate) fn load(&self, id: &str) -> Option<RunRecord> {
        let cache = self.inner.cache.as_ref()?;
        cache.lock().ok()?.load_run(id).ok().flatten()
    }

    /// Commit a legal lifecycle transition before publishing it. Repeated
    /// terminal events and output arriving after a terminal state are inert.
    pub(crate) fn transition(
        &self,
        id: &str,
        status: RunStatus,
        reason: Option<TerminalReason>,
        error: Option<String>,
    ) -> Option<RunRecord> {
        let cache = self.inner.cache.as_ref()?;
        let guard = cache.lock().ok()?;
        let run = guard.load_run(id).ok().flatten()?;
        if run.status.is_terminal() || !run.status.can_transition_to(status) {
            return Some(run);
        }
        let event = if status.is_terminal() {
            RunEvent::Terminated {
                status,
                reason: reason.unwrap_or(TerminalReason::Other),
                message: error.unwrap_or_default(),
            }
        } else {
            RunEvent::State { status }
        };
        let event = NewRunEvent {
            event_id: format!("state-{}", status.name()),
            occurred_at: Timestamp::now(),
            event,
            truncated: false,
        };
        if guard.append_run_event(id, &event).is_err() {
            return None;
        }
        let run = guard.load_run(id).ok().flatten()?;
        drop(guard);
        let _ = self
            .inner
            .events
            .send(Event::AgentRunUpdated { run: run.clone() });
        Some(run)
    }
}

impl Entry {
    async fn wait_done(&self) {
        let mut done = self.done.subscribe();
        while !*done.borrow() {
            if done.changed().await.is_err() {
                break;
            }
        }
    }
}

impl Drop for OwnerDeletion {
    fn drop(&mut self) {
        let Ok(mut gate) = self.manager.inner.admission.lock() else {
            return;
        };
        let Some(count) = gate.blocked.get_mut(&self.fence) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            gate.blocked.remove(&self.fence);
        }
    }
}

fn owner_is_blocked(gate: &AdmissionGate, owner: &RunOwner) -> bool {
    let Some(item_id) = owner.item_id() else {
        return true;
    };
    if gate
        .blocked
        .contains_key(&OwnerFence::Item(item_id.to_string()))
    {
        return true;
    }
    matches!(
        owner,
        RunOwner::Discussion { thread_id, .. }
            if gate.blocked.contains_key(&OwnerFence::Discussion {
                item_id: item_id.to_string(),
                thread_id: thread_id.clone(),
            })
    )
}

fn owner_exists(cache: &perch_github::Cache, owner: &RunOwner) -> Result<bool, String> {
    match owner {
        RunOwner::Discussion {
            item_id,
            thread_id,
            request_id,
        } => {
            if cache
                .load_item(item_id)
                .map_err(|error| error.to_string())?
                .is_none_or(|item| item.is_archived())
            {
                return Ok(false);
            }
            Ok(cache
                .load_comment_threads(item_id)
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|thread| thread.id == *thread_id)
                .is_some_and(|thread| {
                    thread
                        .messages
                        .iter()
                        .any(|message| message.id == *request_id)
                }))
        }
        RunOwner::Review { item_id, .. } => Ok(cache
            .load_item(item_id)
            .map_err(|error| error.to_string())?
            .is_some_and(|item| !item.is_archived())),
        RunOwner::Unavailable { .. } => Ok(false),
    }
}

fn owner_matches(owner: &RunOwner, item_id: &str, thread_id: Option<&str>) -> bool {
    match owner {
        RunOwner::Discussion {
            item_id: owner_item,
            thread_id: owner_thread,
            ..
        } => owner_item == item_id && thread_id.is_none_or(|wanted| wanted == owner_thread),
        RunOwner::Review {
            item_id: owner_item,
            ..
        } => owner_item == item_id && thread_id.is_none(),
        RunOwner::Unavailable { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(limit: usize) -> RunManager {
        let cache = perch_github::Cache::open_in_memory().unwrap();
        let now = Timestamp::now();
        let mut item = perch_github::Item::new("acme/widgets", now);
        item.id = "item".into();
        item.checkout = Some(perch_github::ItemCheckout {
            branch: "feature".into(),
            base_ref: "main".into(),
            head_sha: "abc123".into(),
            path: std::path::PathBuf::from("/tmp/perch-run-manager-test"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        cache.insert_item(&item).unwrap();
        let (events, _) = std::sync::mpsc::channel();
        RunManager::new(limit, Some(Arc::new(Mutex::new(cache))), events.into())
    }

    fn add_request(manager: &RunManager, thread_id: &str, request_id: &str) {
        let cache = manager.inner.cache.as_ref().unwrap();
        let cache = cache.lock().unwrap();
        let has_thread = cache
            .load_comment_threads("item")
            .unwrap()
            .iter()
            .any(|thread| thread.id == thread_id);
        if !has_thread {
            cache
                .insert_comment_thread(&perch_github::CommentThread {
                    id: thread_id.into(),
                    worktree_id: "item".into(),
                    origin: perch_github::ThreadOrigin::Local,
                    github_id: None,
                    anchor: None,
                    outdated: false,
                    resolved: false,
                    node_id: None,
                    can_resolve: false,
                    created_at: Timestamp::now(),
                    messages: Vec::new(),
                })
                .unwrap();
        }
        cache
            .insert_comment_message(&perch_github::CommentMessage {
                id: request_id.into(),
                thread_id: thread_id.into(),
                origin: perch_github::ThreadOrigin::Local,
                author: perch_github::Author::User,
                body: "Why?".into(),
                created_at: Timestamp::now(),
                run: None,
                github_id: None,
                node_id: None,
                html_url: None,
                diff_hunk: None,
                reactions: Default::default(),
            })
            .unwrap();
    }

    fn admit(manager: &RunManager, run: RunRecord) -> Admission {
        let RunOwner::Discussion {
            thread_id,
            request_id,
            ..
        } = &run.owner
        else {
            panic!("test run must be discussion-owned");
        };
        add_request(manager, thread_id, request_id);
        manager.admit(run).unwrap()
    }

    fn run(id: &str, thread: &str, request: &str) -> RunRecord {
        RunRecord {
            id: id.into(),
            owner: RunOwner::Discussion {
                item_id: "item".into(),
                thread_id: thread.into(),
                request_id: request.into(),
            },
            backend: "codex".into(),
            requested_model: "model".into(),
            model: "model".into(),
            requested_effort: "high".into(),
            effort: "high".into(),
            original_request: "Why?".into(),
            queued_at: Timestamp::now(),
            ..RunRecord::default()
        }
    }

    #[tokio::test]
    async fn cancelling_a_queued_run_never_gives_it_the_slot() {
        let manager = manager(1);
        let first = admit(&manager, run("one", "thread-one", "request-one"));
        let first = manager.start(first).await.unwrap();
        let second = admit(&manager, run("two", "thread-two", "request-two"));
        let waiter_manager = manager.clone();
        let waiter = tokio::spawn(async move { waiter_manager.start(second).await.is_some() });

        manager.cancel("two");
        assert!(
            !waiter.await.unwrap(),
            "a cancelled queue entry never acquired a permit"
        );
        assert_eq!(manager.load("two").unwrap().status, RunStatus::Stopped);
        manager.finish(first);
    }

    #[tokio::test]
    async fn scheduler_enforces_the_global_limit_and_fifo_order() {
        let manager = manager(2);
        let first = manager
            .start(admit(&manager, run("one", "thread-one", "request-one")))
            .await
            .unwrap();
        let second = manager
            .start(admit(&manager, run("two", "thread-two", "request-two")))
            .await
            .unwrap();

        let third = admit(&manager, run("three", "thread-three", "request-three"));
        let third_waiter = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.start(third).await.unwrap() })
        };
        tokio::task::yield_now().await;
        let fourth = admit(&manager, run("four", "thread-four", "request-four"));
        let fourth_waiter = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.start(fourth).await.unwrap() })
        };
        tokio::task::yield_now().await;
        assert!(!third_waiter.is_finished());
        assert!(!fourth_waiter.is_finished());

        manager.transition(
            "one",
            RunStatus::Failed,
            Some(TerminalReason::Other),
            Some("fixture complete".into()),
        );
        manager.finish(first);
        let third = tokio::time::timeout(Duration::from_secs(1), third_waiter)
            .await
            .expect("the oldest queued run did not receive the free slot")
            .unwrap();
        assert_eq!(third.entry.id, "three");
        assert!(
            !fourth_waiter.is_finished(),
            "the fourth run bypassed the older waiter"
        );

        manager.transition(
            "two",
            RunStatus::Failed,
            Some(TerminalReason::Other),
            Some("fixture complete".into()),
        );
        manager.finish(second);
        let fourth = tokio::time::timeout(Duration::from_secs(1), fourth_waiter)
            .await
            .expect("the second queued run did not receive the free slot")
            .unwrap();
        assert_eq!(fourth.entry.id, "four");

        for lease in [third, fourth] {
            manager.transition(
                &lease.entry.id,
                RunStatus::Failed,
                Some(TerminalReason::Other),
                Some("fixture complete".into()),
            );
            manager.finish(lease);
        }
    }

    #[tokio::test]
    async fn one_work_run_at_a_time_in_a_worktree_and_questions_go_on_beside_it() {
        let manager = manager(4);
        let work = |id: &str| RunRecord {
            mode: RunMode::Work,
            working_directory: Some("/tmp/perch-run-manager-test".into()),
            ..run(id, &format!("thread-{id}"), &format!("request-{id}"))
        };
        let first = manager.start(admit(&manager, work("one"))).await.unwrap();

        let second = admit(&manager, work("two"));
        let second_waiter = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.start(second).await.unwrap() })
        };
        tokio::task::yield_now().await;
        assert!(
            !second_waiter.is_finished(),
            "slots are free, but the worktree is not"
        );
        assert_eq!(
            manager.load("two").unwrap().latest_activity.as_deref(),
            Some("Waiting for the Work run in this worktree")
        );

        // A question in the same worktree is not held back.
        let ask = manager
            .start(admit(&manager, run("ask", "thread-ask", "request-ask")))
            .await
            .expect("an Ask run starts beside a Work run");

        manager.transition(
            "one",
            RunStatus::Failed,
            Some(TerminalReason::Other),
            Some("fixture complete".into()),
        );
        manager.finish(first);
        let second = tokio::time::timeout(Duration::from_secs(1), second_waiter)
            .await
            .expect("the waiting Work run did not start when the first finished")
            .unwrap();
        assert_eq!(second.entry.id, "two");

        for lease in [ask, second] {
            manager.transition(
                &lease.entry.id,
                RunStatus::Failed,
                Some(TerminalReason::Other),
                Some("fixture complete".into()),
            );
            manager.finish(lease);
        }
    }

    #[tokio::test]
    async fn one_discussion_cannot_admit_two_active_answers() {
        let manager = manager(4);
        let first = admit(&manager, run("one", "thread", "request-one"));
        add_request(&manager, "thread", "request-two");
        let error = manager
            .admit(run("two", "thread", "request-two"))
            .err()
            .unwrap();
        assert!(error.contains("active answer"), "{error}");
        manager.cancel("one");
        assert!(manager.start(first).await.is_none());
    }

    #[tokio::test]
    async fn a_terminal_row_never_receives_a_spawn_lease() {
        let manager = manager(1);
        let admission = admit(&manager, run("one", "thread", "request"));
        manager.transition(
            "one",
            RunStatus::Stopped,
            Some(TerminalReason::QueueCancelled),
            Some("Cancelled before starting".into()),
        );

        assert!(manager.start(admission).await.is_none());
        assert_eq!(manager.load("one").unwrap().status, RunStatus::Stopped);
    }

    #[test]
    fn deletion_fences_admission_and_missing_owners_stay_absent() {
        let manager = manager(1);
        add_request(&manager, "thread", "request");
        let fence = manager.block_owner("item", Some("thread")).unwrap();
        let error = manager
            .admit(run("blocked", "thread", "request"))
            .err()
            .unwrap();
        assert!(error.contains("being deleted"), "{error}");
        assert!(manager.load("blocked").is_none());
        drop(fence);

        let error = manager
            .admit(run("missing", "missing-thread", "missing-request"))
            .err()
            .unwrap();
        assert!(error.contains("is gone"), "{error}");
        assert!(manager.load("missing").is_none());
    }

    #[test]
    fn archived_owners_reject_new_work() {
        let manager = manager(1);
        add_request(&manager, "thread", "request");
        {
            let cache = manager.inner.cache.as_ref().unwrap().lock().unwrap();
            let mut item = cache.load_item("item").unwrap().unwrap();
            item.archived_at = Some(Timestamp::now());
            cache.update_item(&item).unwrap();
        }

        let error = manager
            .admit(run("archived", "thread", "request"))
            .err()
            .unwrap();
        assert!(error.contains("is gone"), "{error}");
        assert!(manager.load("archived").is_none());
    }

    #[tokio::test]
    async fn waiting_for_an_owner_does_not_cancel_its_work() {
        let manager = manager(1);
        let _admission = admit(&manager, run("one", "thread", "request"));
        assert!(manager.has_active_owner("item", None));
        let waiting = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.wait_owner_idle("item", None).await })
        };
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());

        manager.cancel("one");
        waiting.await.unwrap();
        assert!(!manager.has_active_owner("item", None));
    }

    #[tokio::test]
    async fn shutdown_marks_a_queued_run_interrupted_by_the_app() {
        let manager = manager(1);
        let _admission = admit(&manager, run("one", "thread", "request"));

        manager.shutdown().await;

        let run = manager.load("one").unwrap();
        assert_eq!(run.status, RunStatus::Interrupted);
        assert_eq!(run.terminal_reason, Some(TerminalReason::AppShutdown));
    }

    #[tokio::test]
    async fn shutdown_fences_admission_and_waits_for_running_work() {
        let manager = manager(1);
        let admission = admit(&manager, run("one", "thread", "request"));
        let lease = manager.start(admission).await.unwrap();
        manager.transition("one", RunStatus::Running, None, None);

        let shutdown = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.shutdown().await })
        };
        while !manager
            .inner
            .admission
            .lock()
            .map(|gate| gate.closed)
            .unwrap_or(false)
        {
            tokio::task::yield_now().await;
        }
        assert!(
            !shutdown.is_finished(),
            "shutdown returned before the worker"
        );
        let (status, reason) = manager.stop_terminal("one");
        assert_eq!(
            (status, reason),
            (RunStatus::Interrupted, TerminalReason::AppShutdown)
        );
        manager.transition(
            "one",
            status,
            Some(reason),
            Some("Perch shut down while the agent was running".into()),
        );
        manager.finish(lease);
        tokio::time::timeout(Duration::from_secs(1), shutdown)
            .await
            .expect("shutdown did not observe the reaped worker")
            .unwrap();

        add_request(&manager, "later-thread", "later-request");
        let error = manager
            .admit(run("later", "later-thread", "later-request"))
            .err()
            .unwrap();
        assert!(error.contains("shutting down"), "{error}");
        assert!(manager.load("later").is_none());
    }

    #[tokio::test]
    async fn a_failed_terminal_write_keeps_the_live_entry_for_reconciliation() {
        let manager = manager(1);
        let admission = admit(&manager, run("one", "thread", "request"));
        let lease = manager.start(admission).await.unwrap();
        let cache = manager.inner.cache.as_ref().unwrap().clone();
        let poisoned_cache = cache.clone();
        let poisoned = std::thread::spawn(move || {
            let _guard = poisoned_cache.lock().unwrap();
            panic!("poison the cache mutex to model an unavailable terminal write");
        })
        .join();
        assert!(poisoned.is_err());

        manager.finish(lease);

        let entry = manager
            .inner
            .entries
            .lock()
            .unwrap()
            .get("one")
            .cloned()
            .expect("a failed terminal projection must retain its live owner");
        assert!(entry.execution_done.load(Ordering::Acquire));
        assert!(!entry.finished.load(Ordering::Acquire));
        assert!(entry.pending_terminal.lock().unwrap().is_some());

        cache.clear_poison();
        manager.reconcile_finished();
        assert_eq!(manager.load("one").unwrap().status, RunStatus::Failed);
        assert!(!manager.has_active_owner("item", Some("thread")));
    }

    #[tokio::test]
    async fn owner_deletion_uses_the_user_stop_reason_for_started_work() {
        let manager = manager(1);
        let admission = admit(&manager, run("one", "thread", "request"));
        let lease = manager.start(admission).await.unwrap();
        let cancelling = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.cancel_owner("item", Some("thread")).await })
        };
        while manager.load("one").unwrap().status != RunStatus::Stopping {
            tokio::task::yield_now().await;
        }
        let (status, reason) = manager.stop_terminal("one");
        assert_eq!(
            (status, reason),
            (RunStatus::Stopped, TerminalReason::UserStopped)
        );
        manager.transition(
            "one",
            status,
            Some(reason),
            Some("Stopped by the reader".into()),
        );
        manager.finish(lease);
        cancelling.await.unwrap();
        assert_eq!(
            manager.load("one").unwrap().terminal_reason,
            Some(TerminalReason::UserStopped)
        );
    }
}
