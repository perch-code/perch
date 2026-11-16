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
/// back untagged; the engine hands in its connections' router.
pub struct EventSender<T> {
    tx: EventTx<T>,
    window: WindowId,
}

/// What takes a tagged event: Err gives it back when nobody can.
type Route<T> = Arc<dyn Fn(WindowId, T) -> Result<(), T> + Send + Sync>;

enum EventTx<T> {
    Plain(std::sync::mpsc::Sender<T>),
    Routed(Route<T>),
}

impl<T: Send + 'static> EventSender<T> {
    pub fn routed(tx: std::sync::mpsc::Sender<(WindowId, T)>) -> Self {
        Self::to(move |window, event| tx.send((window, event)).map_err(|e| e.0 .1))
    }

    /// Events handed to `route`, each with its window.
    pub fn to(route: impl Fn(WindowId, T) -> Result<(), T> + Send + Sync + 'static) -> Self {
        Self {
            tx: EventTx::Routed(Arc::new(route)),
            window: NO_WINDOW,
        }
    }
}

impl<T> EventSender<T> {
    pub fn send(&self, event: T) -> Result<(), std::sync::mpsc::SendError<T>> {
        match &self.tx {
            EventTx::Plain(tx) => tx.send(event),
            EventTx::Routed(route) => route(self.window, event).map_err(std::sync::mpsc::SendError),
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
            Self::Routed(route) => Self::Routed(route.clone()),
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

/// Where a window's commands go: a connection to the engine, or, for a
/// recording test, a channel it reads back untagged.
#[derive(Clone)]
pub(super) enum CommandSender {
    #[cfg_attr(not(any(test, feature = "test-support")), allow(dead_code))]
    Plain(tokio::sync::mpsc::UnboundedSender<Command>),
    Routed {
        to: Arc<dyn Fn(WindowId, Command) -> Result<(), ()> + Send + Sync>,
        window: WindowId,
    },
}

impl CommandSender {
    pub(super) fn routed(
        to: impl Fn(WindowId, Command) -> Result<(), ()> + Send + Sync + 'static,
    ) -> Self {
        Self::Routed {
            to: Arc::new(to),
            window: NO_WINDOW,
        }
    }

    /// Err when the engine has gone.
    pub(super) fn send(&self, command: Command) -> Result<(), ()> {
        match self {
            Self::Plain(tx) => tx.send(command).map_err(drop),
            Self::Routed { to, window } => to(*window, command),
        }
    }

    pub(super) fn for_window(&self, window: WindowId) -> Self {
        match self {
            Self::Plain(tx) => Self::Plain(tx.clone()),
            Self::Routed { to, .. } => Self::Routed {
                to: to.clone(),
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
    /// The engine's own copy of the window's fences: the handle allocates
    /// the ids, and each command that carries one registers it here when
    /// it arrives.
    pub(super) fn new(window: WindowId) -> Self {
        Self {
            window,
            slot: Default::default(),
            kept_contexts: Default::default(),
            language_servers: Default::default(),
            navigation_requests: Default::default(),
            scope_requests: Default::default(),
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

/// The views and the engine's own fences, which the connections reach as
/// well as the command loop. A command's fences are registered as it is
/// sent, as the handle once registered them as the window clicked, so a
/// task that finishes while the loop is busy with a poll already sees the
/// newer click.
#[derive(Clone, Default)]
pub(super) struct Fencing {
    pub(super) views: Views,
    /// The fences of what runs on no window's view: the engine's own work
    /// and every connection's home.
    pub(super) navigation: Arc<NavigationRequests>,
    pub(super) scopes: Arc<ScopeRequests>,
}

impl Fencing {
    /// Make `window`'s view, unless it has one.
    pub(super) fn open(&self, window: WindowId) {
        if let Ok(mut views) = self.views.lock() {
            if !views.iter().any(|view| view.window == window) {
                views.push(View::new(window));
            }
        }
    }

    /// Register what `command` fences in the fences of the view it runs in.
    pub(super) fn register(&self, window: WindowId, command: &Command) -> super::Fenced {
        let view = self.views.lock().ok().and_then(|views| {
            views.iter().find(|view| view.window == window).map(|view| {
                (
                    view.navigation_requests.clone(),
                    view.scope_requests.clone(),
                )
            })
        });
        let (navigation, scopes) =
            view.unwrap_or_else(|| (self.navigation.clone(), self.scopes.clone()));
        super::register_fences(&navigation, &scopes, command)
    }
}

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
