//! Who is talking to the engine. The engine runs commands from windows
//! and sends events to windows, each tagged with an engine window id. A
//! connection is one client, the app in process or one at the far end of
//! the wire, with window ids of its own that start at 1 like every other
//! client's. This layer maps them both ways.
//!
//! - A command from a connection's window runs as the engine window
//!   that `WindowOpened` gave it.
//! - A command from a connection's `NO_WINDOW` runs as that connection's
//!   home: an engine window with no view, so the engine can give its
//!   answers to that connection alone. Only a window opens or closes a
//!   window.
//! - A `Resync`, from whichever of its windows, runs as the connection's
//!   resync window, whose every event goes to that connection alone, as
//!   its `NO_WINDOW`'s, and nowhere once it has gone.
//! - An event for `NO_WINDOW` goes to every connection.
//! - An event for a window or a home goes to the connection that owns it.
//!   One a client shows in every window (not `Event::for_one_window`)
//!   also goes to every other connection, as their `NO_WINDOW`'s: a
//!   rename asked for in one client is news to the rest.
//!
//! A command's fences are registered as it is sent (`Fencing`), so the
//! engine fences in the order the windows clicked in, whatever its
//! command loop is busy with.
//!
//! When a connection goes, the engine is told each of its windows closed.
//! The engine holds its own command sender, so the last connection going
//! closes nothing, unless the engine was started to live only while
//! connected, as the app's own is.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use tokio::sync::mpsc::UnboundedSender;

use super::windows::Fencing;
use super::{Command, Event, Fenced, WindowId, NO_WINDOW};

/// The bit that marks a connection's resync window, beside its id. Engine
/// windows are counted up from 1 and never get near it.
pub(super) const RESYNC: WindowId = 1 << 62;

/// How long an engine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// Until its last connection closes. The app's own engine: quitting
    /// closes the app's connection, and the engine shuts down as quitting
    /// always has, runs stopped and recorded and children reaped.
    WhileConnected,
    /// Until `Engine::shutdown`, whoever comes and goes: a host's.
    Service,
}

/// What reaches the engine's command loop.
pub(crate) enum Inbound {
    /// A command from an engine window, with what its fences gave back.
    Command(WindowId, Command, Fenced),
    /// Stop as quitting does.
    Shutdown,
}

/// A running engine, or one about to run: what connections are made to.
#[derive(Clone)]
pub struct Engine {
    pub(super) hub: Arc<Hub>,
    pub(super) runtime: tokio::runtime::Handle,
}

impl Engine {
    /// Connect a client. `sink` hears every event meant for it, tagged
    /// with its own window ids; it is called with the engine's routing
    /// held, so it must not block or call back into the engine.
    pub fn connect(&self, sink: impl Fn(WindowId, Event) + Send + Sync + 'static) -> Connection {
        let id = self.hub.add(Box::new(sink));
        Connection {
            hub: self.hub.clone(),
            id,
        }
    }

    /// Stop the engine as quitting the app does.
    pub fn shutdown(&self) {
        let _ = self.hub.commands.send(Inbound::Shutdown);
    }

    /// The engine's runtime, for the tasks that carry a connection.
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.runtime
    }

    /// How many clients are connected.
    pub fn connections(&self) -> usize {
        self.hub.state().connections.len()
    }

    /// How many agent runs are queued or under way: none before the
    /// engine has started them.
    pub fn active_runs(&self) -> usize {
        self.hub.runs.get().map_or(0, |runs| runs.active())
    }

    /// Wait until the engine has stopped: its runs stopped and recorded,
    /// its tasks dropped and its home let go.
    pub fn wait_stopped(&self) {
        let mut stopped = self
            .hub
            .stopped
            .done
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        while !*stopped {
            stopped = self
                .hub
                .stopped
                .changed
                .wait(stopped)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Whether the engine's thread has finished.
#[derive(Default)]
pub(super) struct Stopped {
    done: Mutex<bool>,
    changed: Condvar,
}

/// Says the engine stopped when dropped, so an engine thread that panics
/// still lets its waiters go.
pub(super) struct StopGuard(pub(super) Arc<Hub>);

impl Drop for StopGuard {
    fn drop(&mut self) {
        *self
            .0
            .stopped
            .done
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = true;
        self.0.stopped.changed.notify_all();
    }
}

/// One client's way in. Dropping it closes the client's windows in the
/// engine.
pub struct Connection {
    hub: Arc<Hub>,
    id: u64,
}

impl Connection {
    /// The engine's number for this client, never another's.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Run `command` from the client's `window`. Err when the engine has
    /// gone.
    pub fn send(&self, window: WindowId, command: Command) -> Result<(), ()> {
        self.hub.send(self.id, window, command)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.hub.remove(self.id);
    }
}

type Sink = Box<dyn Fn(WindowId, Event) + Send + Sync>;

/// The connections and their windows.
pub(super) struct Hub {
    commands: UnboundedSender<Inbound>,
    lifetime: Lifetime,
    fencing: Fencing,
    /// Held while a command is mapped, fenced and queued, so the engine
    /// hears commands in the order they were fenced, a window open before
    /// anything from it, and nothing after it closed. Apart from `state`,
    /// which routing takes: the engine routes events while holding a
    /// fence's lock (`NavigationRequests::with_current`), so fencing must
    /// never wait on routing.
    sending: Mutex<()>,
    state: Mutex<HubState>,
    /// The engine's runs, once it has started them, for counting.
    pub(super) runs: OnceLock<super::runs::RunManager>,
    pub(super) stopped: Stopped,
}

#[derive(Default)]
struct HubState {
    /// Engine window ids are handed out here, never reused.
    last_window: WindowId,
    last_connection: u64,
    connections: BTreeMap<u64, Peer>,
    /// Each engine window's connection and that connection's id for it;
    /// `NO_WINDOW` there for a connection's home.
    owners: HashMap<WindowId, (u64, WindowId)>,
}

struct Peer {
    sink: Sink,
    /// The engine window this connection's `NO_WINDOW` runs as.
    home: WindowId,
    /// The connection's windows, to the engine's.
    windows: BTreeMap<WindowId, WindowId>,
}

impl Hub {
    pub(super) fn new(
        commands: UnboundedSender<Inbound>,
        lifetime: Lifetime,
        fencing: Fencing,
    ) -> Self {
        Self {
            commands,
            lifetime,
            fencing,
            sending: Mutex::default(),
            state: Mutex::default(),
            runs: OnceLock::new(),
            stopped: Stopped::default(),
        }
    }

    pub(super) fn lifetime(&self) -> Lifetime {
        self.lifetime
    }

    fn sending(&self) -> std::sync::MutexGuard<'_, ()> {
        self.sending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, HubState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn add(&self, sink: Sink) -> u64 {
        let mut state = self.state();
        state.last_connection += 1;
        let id = state.last_connection;
        let home = state.next_window();
        state.owners.insert(home, (id, NO_WINDOW));
        state.connections.insert(
            id,
            Peer {
                sink,
                home,
                windows: BTreeMap::new(),
            },
        );
        tracing::info!(connection = id, "client connected");
        id
    }

    fn send(&self, id: u64, window: WindowId, command: Command) -> Result<(), ()> {
        let _sending = self.sending();
        let engine_window = {
            let mut state = self.state();
            let state = &mut *state;
            let Some(peer) = state.connections.get_mut(&id) else {
                return Err(());
            };
            match (&command, window) {
                // Asked for from any window, answered to the client alone.
                (Command::Resync, _) => RESYNC | id,
                (Command::WindowOpened | Command::WindowClosed, NO_WINDOW) => {
                    tracing::debug!(connection = id, "a window command from no window; dropped");
                    return Ok(());
                }
                (_, NO_WINDOW) => peer.home,
                (Command::WindowOpened, _) => match peer.windows.get(&window) {
                    Some(&known) => known,
                    None => {
                        state.last_window += 1;
                        let engine_window = state.last_window;
                        peer.windows.insert(window, engine_window);
                        state.owners.insert(engine_window, (id, window));
                        engine_window
                    }
                },
                (Command::WindowClosed, _) => {
                    let Some(engine_window) = peer.windows.remove(&window) else {
                        return Ok(());
                    };
                    state.owners.remove(&engine_window);
                    engine_window
                }
                _ => match peer.windows.get(&window) {
                    Some(&engine_window) => engine_window,
                    None => {
                        tracing::debug!(
                            connection = id,
                            window,
                            "command from a window that is not open; dropped"
                        );
                        return Ok(());
                    }
                },
            }
        };
        // The view is there before anything from the window is fenced.
        if matches!(command, Command::WindowOpened) {
            self.fencing.open(engine_window);
        }
        let fenced = self.fencing.register(engine_window, &command);
        self.commands
            .send(Inbound::Command(engine_window, command, fenced))
            .map_err(drop)
    }

    fn remove(&self, id: u64) {
        let _sending = self.sending();
        let mut state = self.state();
        let Some(peer) = state.connections.remove(&id) else {
            return;
        };
        state.owners.remove(&peer.home);
        for engine_window in peer.windows.into_values() {
            state.owners.remove(&engine_window);
            let _ = self.commands.send(Inbound::Command(
                engine_window,
                Command::WindowClosed,
                Fenced::default(),
            ));
        }
        tracing::info!(
            connection = id,
            left = state.connections.len(),
            "client disconnected"
        );
        if self.lifetime == Lifetime::WhileConnected && state.connections.is_empty() {
            let _ = self.commands.send(Inbound::Shutdown);
        }
    }

    /// Hand an engine event to whoever should hear it.
    pub(super) fn route(&self, window: WindowId, event: Event) {
        let state = self.state();
        // A resync's, for its asker alone; one that has gone hears nothing.
        if window & RESYNC != 0 {
            if let Some(peer) = state.connections.get(&(window & !RESYNC)) {
                (peer.sink)(NO_WINDOW, event);
            }
            return;
        }
        let owner = (window != NO_WINDOW)
            .then(|| state.owners.get(&window).copied())
            .flatten();
        match owner {
            // A window's or a home's own answer: its connection alone.
            Some((id, theirs)) if event.for_one_window() => {
                if let Some(peer) = state.connections.get(&id) {
                    (peer.sink)(theirs, event);
                }
            }
            // News from a window or a home that every client shows.
            Some((id, theirs)) => {
                for (&peer_id, peer) in &state.connections {
                    let to = if peer_id == id { theirs } else { NO_WINDOW };
                    (peer.sink)(to, event.clone());
                }
            }
            // A window or a home that has closed: its answers have nowhere
            // to go, and its news is still news.
            None if window != NO_WINDOW && event.for_one_window() => {}
            None => {
                for peer in state.connections.values() {
                    (peer.sink)(NO_WINDOW, event.clone());
                }
            }
        }
    }
}

impl HubState {
    fn next_window(&mut self) -> WindowId {
        self.last_window += 1;
        self.last_window
    }
}
