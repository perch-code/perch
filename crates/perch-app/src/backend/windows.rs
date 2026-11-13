//! Perch can have several windows, each with an item of its own open. The
//! worker keeps a view per window: the checkout slot, the language server
//! and the navigation sequence there used to be one of. What the windows
//! share, the rows, the inbox and the agents, stays the worker's.
//!
//! Commands come in tagged with the window that sent them, and events go
//! out tagged with the window whose view sent them. The UI decides which
//! events every window hears; see `Event::for_one_window`.

use std::sync::Arc;

use super::{Command, NavigationRequests, ReviewContext, ScopeRequests, Servers};

pub type WindowId = u64;

/// The worker itself, on no window's behalf: the poll, the sweep, startup.
pub const NO_WINDOW: WindowId = 0;

/// Where events go. A test hands in a plain channel and reads the events
/// back untagged.
pub struct EventSender<T> {
    tx: EventTx<T>,
    window: WindowId,
}

enum EventTx<T> {
    Plain(std::sync::mpsc::Sender<T>),
    Routed(std::sync::mpsc::Sender<(WindowId, T)>),
}

impl<T> EventSender<T> {
    pub fn routed(tx: std::sync::mpsc::Sender<(WindowId, T)>) -> Self {
        Self {
            tx: EventTx::Routed(tx),
            window: NO_WINDOW,
        }
    }

    pub fn send(&self, event: T) -> Result<(), std::sync::mpsc::SendError<T>> {
        match &self.tx {
            EventTx::Plain(tx) => tx.send(event),
            EventTx::Routed(tx) => tx
                .send((self.window, event))
                .map_err(|e| std::sync::mpsc::SendError(e.0 .1)),
        }
    }

    /// The same channel, with what is sent on it tagged as `window`'s.
    pub fn for_window(&self, window: WindowId) -> Self {
        Self {
            tx: self.tx.clone(),
            window,
        }
    }
}

impl<T> Clone for EventSender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            window: self.window,
        }
    }
}

impl<T> Clone for EventTx<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Plain(tx) => Self::Plain(tx.clone()),
            Self::Routed(tx) => Self::Routed(tx.clone()),
        }
    }
}

impl<T> From<std::sync::mpsc::Sender<T>> for EventSender<T> {
    fn from(tx: std::sync::mpsc::Sender<T>) -> Self {
        Self {
            tx: EventTx::Plain(tx),
            window: NO_WINDOW,
        }
    }
}

/// Where a window's commands go. A recording test reads them back
/// untagged.
#[derive(Clone)]
pub(super) enum CommandSender {
    Plain(tokio::sync::mpsc::UnboundedSender<Command>),
    Routed {
        tx: tokio::sync::mpsc::UnboundedSender<(WindowId, Command)>,
        window: WindowId,
    },
}

impl CommandSender {
    /// Err when the worker has gone.
    pub(super) fn send(&self, command: Command) -> Result<(), ()> {
        match self {
            Self::Plain(tx) => tx.send(command).map_err(drop),
            Self::Routed { tx, window } => tx.send((*window, command)).map_err(drop),
        }
    }

    pub(super) fn for_window(&self, window: WindowId) -> Self {
        match self {
            Self::Plain(tx) => Self::Plain(tx.clone()),
            Self::Routed { tx, .. } => Self::Routed {
                tx: tx.clone(),
                window,
            },
        }
    }
}

/// One window's side of the worker.
#[derive(Clone)]
pub(super) struct View {
    pub(super) window: WindowId,
    pub(super) slot: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    pub(super) kept_contexts: super::KeptContexts,
    pub(super) language_servers: Servers,
    pub(super) navigation_requests: Arc<NavigationRequests>,
    pub(super) scope_requests: Arc<ScopeRequests>,
    pub(super) review_navigation: Arc<tokio::sync::Mutex<()>>,
    pub(super) window_key: Arc<std::sync::atomic::AtomicBool>,
}

impl View {
    pub(super) fn new(
        window: WindowId,
        navigation_requests: Arc<NavigationRequests>,
        scope_requests: Arc<ScopeRequests>,
    ) -> Self {
        Self {
            window,
            slot: Default::default(),
            kept_contexts: Default::default(),
            language_servers: Default::default(),
            navigation_requests,
            scope_requests,
            review_navigation: Default::default(),
            // A window is made to be looked at.
            window_key: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }

    pub(super) fn open_item(&self) -> Option<String> {
        self.slot
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|open| open.item_id.clone()))
    }
}

/// The windows' views, by window.
pub(super) type Views = Arc<std::sync::Mutex<Vec<View>>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_s_sender_tags_what_it_sends() {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = EventSender::routed(tx);
        let window = worker.for_window(3);
        worker.send("poll").unwrap();
        window.send("hover").unwrap();
        window.clone().send("again").unwrap();
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            vec![(NO_WINDOW, "poll"), (3, "hover"), (3, "again")]
        );
    }
}
