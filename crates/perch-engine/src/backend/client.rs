//! The handle's side of the fences. The handle hands out each window's
//! navigation ids and sends them in commands; the engine keeps a copy of
//! its own and checks it (`NavigationRequests`). The handle needs only
//! the id of the window's latest navigation, for the commands that act on
//! whatever the window shows. The engine moves that back on its own in
//! three places, and says so in an event each time, which the handle
//! reads on its way to the window.
//!
//! Scope picks are numbered here too, by window, as navigations are. A
//! scope pick or a look at the checkout carries the window's latest, and
//! the engine says which it had heard of in each `ScopeReady` and
//! `CommitsReady`. A `ScopeReady` that names an older pick than the
//! window's latest was sent before the engine heard of the newer one, and
//! is dropped here: in process that cannot happen, but over a wire the
//! answer to an earlier look can be on its way as the reader picks.
//! `CommitsReady` is let through: the commits are the same under every
//! scope, its `scope_gen` fences it in the window, and a pick sends none
//! again, so one dropped here would be gone until the next refresh.
//!
//! Merge and review attempts are numbered here too, for the whole client,
//! and must not collide with another client's: their events reach every
//! client, and a window takes the latest it has heard of (see
//! `next_attempt`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::{Event, WindowId};

/// The low bits of an attempt id that name the client's connection.
const CONNECTION_BITS: u32 = 20;

/// Every window's navigation, and the attempts, for one connection.
#[derive(Default)]
pub(crate) struct Navigations {
    windows: Mutex<HashMap<WindowId, Navigation>>,
    /// The engine's number for the connection.
    connection: AtomicU64,
    /// The latest attempt round handed out or heard of, from any client.
    attempt_round: AtomicU64,
}

#[derive(Debug, Default)]
struct Navigation {
    last_id: u64,
    /// The latest navigation, as the engine has it once it has heard
    /// every command sent.
    current: Option<u64>,
    /// A history navigation still loading, and the navigation its failure
    /// goes back to.
    historical: Option<(u64, Option<u64>)>,
    /// The fence each archive was sent with, by item.
    archive_fences: HashMap<String, u64>,
    /// The window's latest scope pick, 0 before its first.
    scope: u64,
}

impl Navigation {
    /// A window's ids start in a range of their own, so no two of a
    /// client's windows ever wait on the same id.
    fn for_window(window: WindowId) -> Self {
        Self {
            last_id: window << 40,
            ..Self::default()
        }
    }

    fn next_id(&mut self) -> u64 {
        self.last_id = self.last_id.saturating_add(1);
        self.last_id
    }
}

impl Navigations {
    pub(crate) fn connected_as(&self, connection: u64) {
        self.connection.store(connection, Ordering::Relaxed);
    }

    /// A merge or review attempt's id: unique in the engine, by the
    /// connection in its low bits, and after every attempt this client
    /// has heard of from any other, so a window that took another
    /// client's attempt as its latest takes this one's after it.
    pub(super) fn next_attempt(&self) -> u64 {
        let round = self
            .attempt_round
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let connection = self.connection.load(Ordering::Relaxed) & ((1 << CONNECTION_BITS) - 1);
        (round << CONNECTION_BITS) | connection
    }

    fn heard_attempt(&self, attempt_id: u64) {
        self.attempt_round
            .fetch_max(attempt_id >> CONNECTION_BITS, Ordering::Relaxed);
    }

    fn with<T>(&self, window: WindowId, f: impl FnOnce(&mut Navigation) -> T) -> T {
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(windows
            .entry(window)
            .or_insert_with(|| Navigation::for_window(window)))
    }

    pub(super) fn forget(&self, window: WindowId) {
        if let Ok(mut windows) = self.windows.lock() {
            windows.remove(&window);
        }
    }

    /// A navigation to a live view: an item, a subject, Close.
    pub(super) fn begin(&self, window: WindowId) -> u64 {
        self.with(window, |nav| {
            let id = nav.next_id();
            nav.current = Some(id);
            nav.historical = None;
            id
        })
    }

    /// A navigation to a review's history, which a failure undoes. A
    /// second one before the first loads goes back where the first would.
    pub(super) fn begin_historical(&self, window: WindowId) -> u64 {
        self.with(window, |nav| {
            let rollback = match nav.historical {
                Some((pending, rollback)) if nav.current == Some(pending) => rollback,
                _ => nav.current,
            };
            let id = nav.next_id();
            nav.current = Some(id);
            nav.historical = Some((id, rollback));
            id
        })
    }

    /// An archive's fence: an id after every navigation so far.
    pub(super) fn archive_fence(&self, window: WindowId, item_id: &str) -> u64 {
        self.with(window, |nav| {
            let id = nav.next_id();
            nav.archive_fences.insert(item_id.to_string(), id);
            id
        })
    }

    /// The archive was never sent.
    pub(super) fn drop_archive_fence(&self, window: WindowId, item_id: &str) {
        self.with(window, |nav| nav.archive_fences.remove(item_id));
    }

    /// A scope pick: an id after every one the window has had.
    pub(super) fn begin_scope(&self, window: WindowId) -> u64 {
        self.with(window, |nav| {
            let id = nav.next_id();
            nav.scope = id;
            id
        })
    }

    /// The window's latest scope pick, which a look at the checkout is
    /// made under.
    pub(super) fn scope(&self, window: WindowId) -> u64 {
        self.lookup(window, |nav| nav.scope).unwrap_or(0)
    }

    /// The engine has gone and another one, or the same one again, is
    /// connected: it has heard of none of the windows' scope picks, so
    /// what it says carries none, and must not be taken for stale.
    pub(crate) fn reconnected(&self, connection: u64) {
        self.connected_as(connection);
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for nav in windows.values_mut() {
            nav.scope = 0;
        }
    }

    pub(super) fn current(&self, window: WindowId) -> Option<u64> {
        self.lookup(window, |nav| nav.current).flatten()
    }

    /// A window that has navigated, and is not forgotten.
    fn lookup<T>(&self, window: WindowId, f: impl FnOnce(&mut Navigation) -> T) -> Option<T> {
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        windows.get_mut(&window).map(f)
    }

    /// Whether `event` is still news to `window`: not an answer about the
    /// checkout made before the engine heard of the window's latest scope
    /// pick. Every event goes past `observe` either way.
    pub(crate) fn pass(&self, window: WindowId, event: &Event) -> bool {
        self.observe(window, event);
        match event {
            Event::ScopeReady { scope_id, .. } => self
                .lookup(window, |nav| *scope_id >= nav.scope)
                .unwrap_or(true),
            _ => true,
        }
    }

    /// Follow the engine where it moves a window's navigation itself, and
    /// keep the attempts after every one heard of. A window with nothing
    /// here has nothing to move: an event that arrives after it closed
    /// leaves no entry behind.
    pub(crate) fn observe(&self, window: WindowId, event: &Event) {
        match event {
            Event::PullRequestMergeStarted { attempt_id, .. }
            | Event::PullRequestMergeFailed { attempt_id, .. }
            | Event::PullRequestReviewStarted { attempt_id, .. }
            | Event::PullRequestReviewSubmitted { attempt_id, .. }
            | Event::PullRequestReviewFailed { attempt_id, .. }
            | Event::PullRequestUpdated {
                merge_attempt_id: Some(attempt_id),
                ..
            } => self.heard_attempt(*attempt_id),
            _ => {}
        }
        let with = |f: &dyn Fn(&mut Navigation)| {
            self.lookup(window, f);
        };
        match event {
            // An attempt that opened its owner fails back to that owner.
            Event::AgentReviewNavigationReady {
                request_id,
                owner_changed: true,
                ..
            } => with(&|nav| {
                if let Some((pending, rollback)) = nav.historical.as_mut() {
                    if pending == request_id {
                        *rollback = Some(*request_id);
                    }
                }
            }),
            Event::HistoricalReviewReady { request_id, .. } => with(&|nav| {
                if nav
                    .historical
                    .is_some_and(|(pending, _)| pending == *request_id)
                {
                    nav.historical = None;
                }
            }),
            Event::HistoricalReviewFailed { request_id, .. }
            | Event::AgentReviewNavigationFailed { request_id, .. } => with(&|nav| {
                if let Some((pending, rollback)) = nav.historical {
                    if pending == *request_id && nav.current == Some(pending) {
                        nav.current = rollback;
                        nav.historical = None;
                    }
                }
            }),
            // An archive of what the window showed moves it to the fence.
            Event::ItemArchived {
                item_id,
                cancelled_navigation_id,
            } => with(&|nav| {
                let fence = nav.archive_fences.remove(item_id);
                if let (Some(fence), Some(cancelled)) = (fence, cancelled_navigation_id) {
                    if nav.current == Some(*cancelled) {
                        nav.current = Some(fence);
                        nav.historical = None;
                    }
                }
            }),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_history_goes_back_to_the_navigation_before_it() {
        let navigations = Navigations::default();
        let live = navigations.begin(1);
        let first = navigations.begin_historical(1);
        let second = navigations.begin_historical(1);
        // The first one's failure is stale: the second is under way.
        navigations.observe(
            1,
            &Event::HistoricalReviewFailed {
                item_id: "item".into(),
                group_id: "g".into(),
                request_id: first,
                message: String::new(),
            },
        );
        assert_eq!(navigations.current(1), Some(second));
        navigations.observe(
            1,
            &Event::HistoricalReviewFailed {
                item_id: "item".into(),
                group_id: "g".into(),
                request_id: second,
                message: String::new(),
            },
        );
        assert_eq!(navigations.current(1), Some(live));
    }

    #[test]
    fn an_archive_of_the_open_item_moves_to_its_fence() {
        let navigations = Navigations::default();
        let open = navigations.begin(2);
        let fence = navigations.archive_fence(2, "item");
        assert!(fence > open);
        assert_eq!(navigations.current(2), Some(open));
        navigations.observe(
            2,
            &Event::ItemArchived {
                item_id: "item".into(),
                cancelled_navigation_id: Some(open),
            },
        );
        assert_eq!(navigations.current(2), Some(fence));
        // Another window's ids are its own.
        assert!(navigations.begin(3) > 3 << 40);
    }

    fn scope_ready(scope_id: u64) -> Event {
        Event::ScopeReady {
            navigation_id: 1,
            scope_gen: 1,
            scope_id,
            item_id: "item".into(),
            scope: perch_git::Scope::Branch,
            files: Default::default(),
            reread_all: false,
        }
    }

    fn commits_ready(scope_id: u64) -> Event {
        Event::CommitsReady {
            navigation_id: 1,
            scope_gen: 1,
            scope_id,
            item_id: "item".into(),
            commits: Default::default(),
            dirt: Default::default(),
            since: None,
            last_run: Default::default(),
        }
    }

    #[test]
    fn an_answer_from_before_the_engine_heard_of_the_latest_scope_pick_is_dropped() {
        let navigations = Navigations::default();
        navigations.begin(1);
        // Nothing picked yet: whatever the engine says goes through.
        assert!(navigations.pass(1, &scope_ready(0)));
        let first = navigations.begin_scope(1);
        assert_eq!(navigations.scope(1), first);
        assert!(navigations.pass(1, &scope_ready(first)));
        let second = navigations.begin_scope(1);
        assert!(second > first);
        // A look the engine finished before it heard of the second pick.
        assert!(!navigations.pass(1, &scope_ready(first)));
        // The commits are the same under any scope, and none come again.
        assert!(navigations.pass(1, &commits_ready(first)));
        assert!(navigations.pass(1, &scope_ready(second)));
        assert!(navigations.pass(1, &commits_ready(second)));
        // Another window's picks are its own.
        assert!(navigations.pass(2, &scope_ready(0)));
        // A new engine has heard of no pick: what it says is not stale.
        navigations.reconnected(7);
        assert!(navigations.pass(1, &scope_ready(0)));
        assert!(navigations.next_attempt() & ((1 << CONNECTION_BITS) - 1) == 7);
    }

    #[test]
    fn a_late_event_for_a_closed_window_leaves_nothing_behind() {
        let navigations = Navigations::default();
        navigations.begin(2);
        navigations.forget(2);
        navigations.observe(
            2,
            &Event::ItemArchived {
                item_id: "item".into(),
                cancelled_navigation_id: None,
            },
        );
        assert_eq!(navigations.current(2), None);
        assert!(navigations.windows.lock().unwrap().is_empty());
    }
}
