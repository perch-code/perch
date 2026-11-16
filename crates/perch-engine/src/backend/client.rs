//! The handle's side of the fences. The handle hands out each window's
//! navigation ids and sends them in commands; the engine keeps a copy of
//! its own and checks it (`NavigationRequests`). The handle needs only
//! the id of the window's latest navigation, for the commands that act on
//! whatever the window shows. The engine moves that back on its own in
//! three places, and says so in an event each time, which the handle
//! reads on its way to the window.

use std::collections::HashMap;
use std::sync::Mutex;

use super::{Event, WindowId};

/// Every window's navigation, for one connection.
#[derive(Default)]
pub(crate) struct Navigations {
    windows: Mutex<HashMap<WindowId, Navigation>>,
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

    pub(super) fn current(&self, window: WindowId) -> Option<u64> {
        self.with(window, |nav| nav.current)
    }

    /// Follow the engine where it moves a window's navigation itself.
    pub(crate) fn observe(&self, window: WindowId, event: &Event) {
        match event {
            // An attempt that opened its owner fails back to that owner.
            Event::AgentReviewNavigationReady {
                request_id,
                owner_changed: true,
                ..
            } => self.with(window, |nav| {
                if let Some((pending, rollback)) = nav.historical.as_mut() {
                    if pending == request_id {
                        *rollback = Some(*request_id);
                    }
                }
            }),
            Event::HistoricalReviewReady { request_id, .. } => self.with(window, |nav| {
                if nav
                    .historical
                    .is_some_and(|(pending, _)| pending == *request_id)
                {
                    nav.historical = None;
                }
            }),
            Event::HistoricalReviewFailed { request_id, .. }
            | Event::AgentReviewNavigationFailed { request_id, .. } => self.with(window, |nav| {
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
            } => self.with(window, |nav| {
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
}
