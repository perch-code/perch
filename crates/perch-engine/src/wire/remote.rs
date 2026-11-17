//! The app's engine on another machine, reached by `ssh <host> perchd
//! bridge`. The connection can be lost, a laptop closed or a network
//! changed, and the app goes on with what it had, says so, and makes the
//! connection again.
//!
//! - The handle refuses a command while there is no connection, with a
//!   notice in the window that sent it. Nothing is kept to send later: a
//!   click made against what the window showed then could mean something
//!   else by the time the engine hears it. The one exception is the app
//!   starting: what its first windows ask for as they are made goes once
//!   the first try connects, and is dropped if it fails. Nothing was lost
//!   then, and a window that asked for its lists and heard nothing would
//!   sit empty. Each window whose asks were dropped is told so. Nor is a
//!   window told off for what the reader did not do: which window has the
//!   keyboard is kept and said once connected, and a comment's picture is
//!   asked for again by the app.
//! - Tries are made 1 s apart at first, doubling to 30 s between, and the
//!   reader can ask for one now, which gives up a try under way. A try
//!   that has no answer in [`Backoff::answer`] is given up: a host that
//!   does not answer, or a login shell that ate the hello. A connection
//!   that ends within [`Backoff::steady`] of being made does not start the
//!   waits again, so an engine that fails at once is not restarted every
//!   second. A refusal of the app's protocol stops the tries: trying again
//!   cannot help until one side is updated.
//! - A connection made asks for everything again (`Command::Resync`), after
//!   telling the engine of every window open and which of them is key, so
//!   the windows catch up on
//!   what happened meanwhile. Each window reopens its item when it hears
//!   it is connected and something it asked for may have been lost
//!   (`EngineLink::Up`'s `again`).
//!
//! - With no `[remote] command`, a try that ends with the host saying it
//!   has no `perchd` of the app's build sends it and tries again at once,
//!   outside the waits (see `provision`). Sending once is all a try gets:
//!   a host that still says so after is a fault the reader has to look
//!   at, and the tries stop.
//! - With no `[remote] command`, an engine of another build than the
//!   app's, already running there, is replaced when it has nothing
//!   running, and otherwise left running with a restart offered (see
//!   `keep_connected`). Builds are told apart, never ordered: the app's
//!   is the one it carries.
//!
//! - Perch's own updates are this Mac's to look for and install: the
//!   engine on the host has no app here to replace. Check for Updates…
//!   and Restart to update never go to it, connected or not; the app asks
//!   GitHub itself, with no token, and the clock that looks every six
//!   hours runs here (`over_ssh`). The next connection after the app
//!   updates sends the `perchd` the new app carries.
//!
//! How the connection stands goes to the windows as `Event::Link`, with
//! the engine's own events, in order.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::UnboundedSender;

use super::provision::{self, Failed, Platform, Replace, Source};
use super::{handshake, WireError, PROTOCOL, VERSION};

/// The builds of `perchd` a dial may have run on the host: the app's, one
/// for each platform it has an engine for.
pub type Wanted = Vec<String>;
use crate::backend::client::Navigations;
use crate::backend::update::{self, Releases};
use crate::backend::{
    Backend, Command, EngineLink, Event, EventSender, OtherEngine, WindowId, NO_WINDOW,
};

/// How often a refused command is said, at most: a pointer moving over
/// code asks for hovers many times a second.
const REFUSALS_APART: Duration = Duration::from_secs(2);

/// How much longer a try that replaces the engine waits for an answer: the
/// bridge gives the engine it stops a minute to stop its runs and let the
/// home go.
const REPLACING: Duration = Duration::from_secs(60);

/// How long a try made as `replace` says waits for the engine's answer.
fn answer_wait(answer: Duration, replace: Replace) -> Duration {
    match replace {
        Replace::No => answer,
        Replace::IfIdle | Replace::Always => answer + REPLACING,
    }
}

/// What ssh runs on the host to reach the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bridge {
    /// `[remote] command`, run in the login shell as given. Nothing is
    /// sent.
    Command(String),
    /// The app's own `perchd`, from `Source`, sent there when it is not
    /// there already.
    Installed(Source),
}

/// A byte stream to an engine.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Stream for T {}

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A stream to an engine, as a dial made it.
pub struct Dialed {
    pub stream: Box<dyn Stream>,
    /// The builds of `perchd` the dial would run, whichever the host is,
    /// when it is the app's own; None for a `[remote] command`, which may
    /// run any.
    pub build: Option<Wanted>,
    /// Let go of what carries the stream, once the stream is done with,
    /// and say what it said last on its way out, such as ssh's last line
    /// of stderr: why it ended, most often.
    pub hang_up: Box<dyn FnOnce() -> BoxFuture<Option<String>> + Send>,
}

/// How a stream to the engine is made: `ssh` in the app, a pipe in memory
/// in a test, replacing an engine of another build as asked. Err says why
/// none could be.
pub type Dial = Box<dyn FnMut(Replace) -> BoxFuture<Result<Dialed, String>> + Send>;

/// How `perchd` is put on the host, for a try that ended with the host
/// saying it has none, for the platform it named: what it sends, as the
/// windows say it, said at once, and the sending. The watch sender carries the latest phase
/// and transfer counts without queuing every network chunk.
pub type Install =
    Box<dyn FnMut(Platform, provision::Progress) -> (String, BoxFuture<Result<(), Failed>>) + Send>;

/// The waits between tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub first: Duration,
    pub last: Duration,
    /// How long a try waits for the engine's answer before it is given up.
    /// Past ssh's own connect timeout, and past a bridge starting an engine.
    pub answer: Duration,
    /// How long a connection has to have lasted for the waits to start
    /// again from `first` once it ends.
    pub steady: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            first: Duration::from_secs(1),
            last: Duration::from_secs(30),
            answer: Duration::from_secs(30),
            steady: Duration::from_secs(60),
        }
    }
}

impl Backoff {
    /// The wait after one of `wait`.
    pub fn after(&self, wait: Duration) -> Duration {
        wait.saturating_mul(2).min(self.last)
    }
}

/// The connection, shared by the handles and the task that keeps it up.
pub struct Link {
    /// The host as the reader named it, which the windows show.
    host: String,
    navigations: Arc<Navigations>,
    events: std::sync::mpsc::Sender<(WindowId, Event)>,
    state: Mutex<Linked>,
    /// Told to try now rather than when the wait is up.
    retry: tokio::sync::Notify,
    /// Told when the reader asks for the restart offered.
    restart: tokio::sync::Notify,
    /// What the app's own updates run on, and ask GitHub through.
    runtime: tokio::runtime::Handle,
    releases: Releases,
}

struct Linked {
    /// Where commands go while connected.
    to: Option<UnboundedSender<(WindowId, Command)>>,
    /// The client's windows that are open, to open again in the engine
    /// each time it is connected, and whether each is the key window. The
    /// engine takes a window it is told of as key until told otherwise.
    windows: BTreeMap<WindowId, bool>,
    /// When a refusal was last said to each window.
    refused_at: HashMap<WindowId, Instant>,
    /// What the windows sent before the first try ended, while it is
    /// under way; `None` after.
    starting: Option<Vec<(WindowId, Command)>>,
    /// The engine of another build the windows offer to restart, while
    /// they do.
    offer: Option<OtherEngine>,
    /// The build the reader asked a restart for, while it is not yet
    /// under way.
    restart: Option<String>,
}

impl Default for Linked {
    fn default() -> Self {
        Linked {
            to: None,
            windows: BTreeMap::new(),
            refused_at: HashMap::new(),
            starting: Some(Vec::new()),
            offer: None,
            restart: None,
        }
    }
}

impl Link {
    fn linked(&self) -> std::sync::MutexGuard<'_, Linked> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn navigations(&self) -> Arc<Navigations> {
        self.navigations.clone()
    }

    /// Send a window's command, or refuse it, saying so, with no
    /// connection. A window opening, closing or becoming key is kept
    /// either way: the reader did none of those against the engine.
    pub(crate) fn send(&self, window: WindowId, command: Command) -> Result<(), ()> {
        let command = match self.update_here(window, command) {
            Some(command) => command,
            None => return Ok(()),
        };
        let mut linked = self.linked();
        match command {
            Command::WindowOpened if window != NO_WINDOW => {
                linked.windows.insert(window, true);
            }
            Command::WindowClosed => {
                linked.windows.remove(&window);
            }
            Command::WindowKey { key } => {
                if let Some(was) = linked.windows.get_mut(&window) {
                    *was = key;
                }
            }
            _ => {}
        }
        let command = match &linked.to {
            Some(to) => match to.send((window, command)) {
                Ok(()) => return Ok(()),
                // The connection has just gone; the task that keeps it
                // hears so too.
                Err(tokio::sync::mpsc::error::SendError((_, command))) => {
                    linked.to = None;
                    command
                }
            },
            None => command,
        };
        // Sent when the connection is made, whenever that is.
        if matches!(
            command,
            Command::WindowOpened
                | Command::WindowClosed
                | Command::WindowKey { .. }
                | Command::Resync
        ) {
            return Ok(());
        }
        if let Some(starting) = linked.starting.as_mut() {
            starting.push((window, command));
            return Ok(());
        }
        // A picture is asked for as a comment is drawn, not by the reader,
        // and asked for again once connected.
        if !matches!(command, Command::WantPicture { .. }) {
            self.refuse(&mut linked, window);
        }
        Err(())
    }

    /// Look for or install an update of this app here, rather than send
    /// the command to the engine on the host, which would look for one of
    /// its own. Any other command comes back.
    fn update_here(&self, window: WindowId, command: Command) -> Option<Command> {
        let events = EventSender::routed(self.events.clone()).for_window(window);
        let releases = self.releases.clone();
        match command {
            Command::CheckForUpdate => {
                self.runtime
                    .spawn(async move { update::check_now(&releases, &events).await });
            }
            Command::InstallUpdate { version } => {
                self.runtime
                    .spawn(async move { update::install(&releases, &events, version).await });
            }
            command => return Some(command),
        }
        None
    }

    /// Tell `window` a command of its was not done, unless it was told
    /// just now.
    fn refuse(&self, linked: &mut Linked, window: WindowId) {
        let now = Instant::now();
        if linked
            .refused_at
            .get(&window)
            .is_none_or(|at| now.duration_since(*at) >= REFUSALS_APART)
        {
            linked.refused_at.insert(window, now);
            let _ = self
                .events
                .send((window, Event::Notice(refused(&self.host))));
        }
    }

    /// Try now, unless connected.
    pub(crate) fn retry_now(&self) {
        if self.linked().to.is_none() {
            self.retry.notify_one();
        }
    }

    /// Restart the engine offered, unless none is: the strip's Restart.
    pub(crate) fn restart_now(&self) {
        let mut linked = self.linked();
        if let Some(offer) = linked.offer.take() {
            linked.restart = Some(offer.wanted);
            drop(linked);
            self.restart.notify_one();
        }
    }

    /// The build of the restart the reader asked for, which this takes,
    /// saying it is under way.
    fn take_restart(&self) -> Option<String> {
        let build = self.linked().restart.take()?;
        tracing::info!(host = self.host, build, "restarting the engine, as asked");
        self.say(EngineLink::Restarting {
            host: self.host.clone(),
            build: build.clone(),
        });
        Some(build)
    }

    /// How the try after a wake the reader asked for treats the engine.
    fn after_wake(&self) -> Replace {
        match self.take_restart() {
            Some(_) => Replace::Always,
            None => Replace::No,
        }
    }

    /// Offer to restart `other`, an engine of another build left running.
    fn offering(&self, linked: &mut Linked, other: Option<&OtherEngine>) {
        linked.offer = other.cloned();
    }

    /// Connected: the windows are opened in the engine, each said key or
    /// not as it last was, then everything is asked for again, before any
    /// command from a window can go. `other` when the engine is another
    /// build, left running.
    fn up(
        &self,
        welcome: &super::Welcome,
        to: UnboundedSender<(WindowId, Command)>,
        other: Option<OtherEngine>,
    ) -> bool {
        let mut linked = self.linked();
        self.offering(&mut linked, other.as_ref());
        self.navigations.reconnected(welcome.connection);
        for (&window, &key) in &linked.windows {
            let _ = to.send((window, Command::WindowOpened));
            let _ = to.send((window, Command::WindowKey { key }));
        }
        let _ = to.send((NO_WINDOW, Command::Resync));
        // Only the first try has what the windows asked for as they were
        // made. Any later one may have had commands refused or lost.
        let starting = linked.starting.take();
        let again = starting.is_none();
        for sent in starting.into_iter().flatten() {
            let _ = to.send(sent);
        }
        linked.to = Some(to);
        linked.refused_at.clear();
        self.say(EngineLink::Up {
            host: self.host.clone(),
            welcome: welcome.clone(),
            again,
            other,
        })
    }

    /// Not connected. What the windows asked for while the first try was
    /// under way is dropped, and each window that asked is told.
    fn down(&self) {
        let mut linked = self.linked();
        linked.to = None;
        linked.offer = None;
        let dropped = linked.starting.take().unwrap_or_default();
        let windows: BTreeSet<WindowId> = dropped.into_iter().map(|(window, _)| window).collect();
        for window in windows {
            self.refuse(&mut linked, window);
        }
    }

    /// Tell the windows how the connection stands. False once nobody is
    /// listening: the app has gone.
    fn say(&self, link: EngineLink) -> bool {
        self.events.send((NO_WINDOW, Event::Link(link))).is_ok()
    }

    /// Not connected for `why`, which trying again cannot help: no more
    /// tries until the reader asks for one, or for the restart of `other`
    /// offered. False once nobody is listening.
    async fn halt(&self, why: String, ever: bool, other: Option<OtherEngine>) -> bool {
        self.down();
        self.offering(&mut self.linked(), other.as_ref());
        if !self.say(EngineLink::Down {
            host: self.host.clone(),
            why,
            retry_in: None,
            ever,
            other,
        }) {
            return false;
        }
        tokio::select! {
            () = self.retry.notified() => {}
            () = self.restart.notified() => {}
        }
        true
    }
}

/// What a window is told of a command refused while disconnected.
pub fn refused(host: &str) -> String {
    format!("Not connected to {host}, so that wasn't done. Perch is reconnecting.")
}

/// Connect to an engine through `dial`, and keep connected, on `runtime`.
/// Returns at once, before the first connection is made: the handle
/// refuses commands until then, and the events say how it stands.
pub fn link(
    host: &str,
    dial: Dial,
    install: Option<Install>,
    backoff: Backoff,
    runtime: &tokio::runtime::Handle,
) -> (Backend, std::sync::mpsc::Receiver<(WindowId, Event)>) {
    let (link, received) = start(host, dial, install, backoff, runtime);
    (Backend::linked(link), received)
}

/// What `link` does, with the connection kept for `over_ssh`.
fn start(
    host: &str,
    dial: Dial,
    install: Option<Install>,
    backoff: Backoff,
    runtime: &tokio::runtime::Handle,
) -> (Arc<Link>, std::sync::mpsc::Receiver<(WindowId, Event)>) {
    let (events, received) = std::sync::mpsc::channel();
    let link = Arc::new(Link {
        host: host.to_string(),
        navigations: Arc::default(),
        events,
        state: Mutex::default(),
        retry: tokio::sync::Notify::new(),
        restart: tokio::sync::Notify::new(),
        runtime: runtime.clone(),
        releases: Releases::anonymous(),
    });
    runtime.spawn(keep_connected(link.clone(), dial, install, backoff));
    (link, received)
}

/// Make the connection, and make it again each time it is lost, until the
/// app stops listening.
///
/// With the app's own `perchd`, the engine reached is weighed against the
/// app's build (`judge`). Another build with nothing running is replaced
/// at once, once a connect: the next dial passes `--replace-if-idle`. One
/// busy is left running, and the windows offer a restart, which dials
/// with `--replace`.
async fn keep_connected(
    link: Arc<Link>,
    mut dial: Dial,
    mut install: Option<Install>,
    backoff: Backoff,
) {
    let host = link.host.clone();
    if !link.say(EngineLink::Connecting { host: host.clone() }) {
        return;
    }
    let mut wait = backoff.first;
    let mut ever = false;
    // What the last try sent, while the try after it is made.
    let mut installed: Option<String> = None;
    // How the next dial treats an engine of another build.
    let mut replace = Replace::No;
    // Whether this connect has replaced an engine by itself: it does so
    // once, so two builds can never take turns stopping each other.
    let mut replaced = false;
    // The engine an automatic replace was for, which a busy line from
    // that dial is about.
    let mut replacing: Option<Replacing> = None;
    loop {
        let just_installed = installed.take();
        let asked = std::mem::take(&mut replace);
        // A dial can take a while by itself, when it has the host ready
        // its own `perchd` first, so the reader's try now gives it up
        // too: left waiting, the ask would end the connection it made.
        let dialed = tokio::select! {
            dialed = dial(asked) => Some(dialed),
            () = link.retry.notified() => None,
        };
        // Whether the try ended before an engine answered it: ssh never
        // reached the host, or the bridge there failed. What the try was
        // to replace is not known to have been, so the next try is still
        // asked to. Asking again is harmless: an engine of the app's own
        // build is joined as it is.
        let mut unanswered = false;
        let why = match dialed {
            None => {
                tracing::info!(host, "trying again now, as asked");
                (replace, installed) = (asked, just_installed);
                continue;
            }
            Some(Err(why)) => {
                unanswered = true;
                why
            }
            Some(Ok(Dialed {
                stream,
                hang_up,
                build: wanted,
            })) => {
                // Given up when the reader asks for a try now, or when the
                // engine is too long answering.
                let within = answer_wait(backoff.answer, asked);
                let answered = tokio::select! {
                    answered = tokio::time::timeout(within, handshake(stream)) => Some(answered),
                    () = link.retry.notified() => None,
                };
                match answered {
                    None => {
                        let _ = hang_up().await;
                        tracing::info!(host, "trying again now, as asked");
                        (replace, installed) = (asked, just_installed);
                        continue;
                    }
                    Some(Err(_)) => {
                        unanswered = true;
                        hang_up().await.unwrap_or_else(|| {
                            format!("{host} did not answer within {} s.", within.as_secs())
                        })
                    }
                    Some(Ok(Ok((welcome, session)))) => {
                        let other = match judge(wanted.as_deref(), &welcome.about(), !replaced) {
                            Judged::Replace(other) => {
                                drop(session);
                                let _ = hang_up().await;
                                tracing::info!(
                                    host,
                                    engine = other.engine,
                                    wanted = other.wanted,
                                    "the engine is another build with nothing running; replacing it"
                                );
                                if !link.say(EngineLink::Restarting {
                                    host: host.clone(),
                                    build: other.wanted.clone(),
                                }) {
                                    return;
                                }
                                (replace, replaced) = (Replace::IfIdle, true);
                                replacing = Some(Replacing {
                                    other,
                                    welcomed: true,
                                });
                                continue;
                            }
                            Judged::Leave(other) => {
                                tracing::info!(
                                    host,
                                    engine = other.engine,
                                    wanted = other.wanted,
                                    runs = other.runs,
                                    clients = other.clients,
                                    "the engine is another build; leaving it running"
                                );
                                Some(other)
                            }
                            Judged::Use => {
                                replaced = false;
                                None
                            }
                        };
                        tracing::info!(
                            host,
                            engine = welcome.engine_version,
                            build = welcome.build,
                            engine_host = welcome.host,
                            "connected to the engine"
                        );
                        let (to, outgoing) = tokio::sync::mpsc::unbounded_channel();
                        let mut ended =
                            session.carry(outgoing, link.navigations(), link.events.clone());
                        if !link.up(&welcome, to, other) {
                            return;
                        }
                        ever = true;
                        let since = Instant::now();
                        // Until the engine goes, or the reader asks for the
                        // restart offered.
                        let restart = loop {
                            tokio::select! {
                                _ = &mut ended => break false,
                                () = link.restart.notified() => {
                                    if link.linked().restart.is_some() {
                                        break true;
                                    }
                                }
                            }
                        };
                        // A connection that lasted starts the waits again;
                        // one that ended at once, an engine failing as it
                        // is asked for everything say, keeps doubling them.
                        if since.elapsed() >= backoff.steady {
                            wait = backoff.first;
                        }
                        link.down();
                        if restart {
                            // Said before the hang-up, which can take a
                            // moment, so the strip changes on the click.
                            replace = link.after_wake();
                            let _ = hang_up().await;
                            continue;
                        }
                        hang_up()
                            .await
                            .unwrap_or_else(|| "The connection was lost.".to_string())
                    }
                    Some(Ok(Err(WireError::Refused(reason, about)))) => {
                        let _ = hang_up().await;
                        tracing::warn!(host, %reason, "the engine refused the app");
                        let (why, other) = match judge(wanted.as_deref(), &about, !replaced) {
                            // An engine from before the handshake was
                            // frozen cannot be asked to stop.
                            _ if wanted.is_some() && about.protocol == 0 => (
                                format!(
                                    "{reason} Perch can't replace an engine this old: stop perchd serve on {host} and reconnect."
                                ),
                                None,
                            ),
                            Judged::Replace(other) => {
                                tracing::info!(
                                    host,
                                    engine = other.engine,
                                    wanted = other.wanted,
                                    "the engine is another build with nothing running; replacing it"
                                );
                                if !link.say(EngineLink::Restarting {
                                    host: host.clone(),
                                    build: other.wanted.clone(),
                                }) {
                                    return;
                                }
                                (replace, replaced) = (Replace::IfIdle, true);
                                replacing = Some(Replacing {
                                    other,
                                    welcomed: false,
                                });
                                continue;
                            }
                            Judged::Leave(other) => (
                                format!(
                                    "It speaks protocol {}, and this app {PROTOCOL}, so Perch can't use it until it restarts.",
                                    about.protocol
                                ),
                                Some(other),
                            ),
                            Judged::Use => (reason, None),
                        };
                        if !link.halt(why, ever, other).await {
                            return;
                        }
                        (replace, replaced) = (link.after_wake(), false);
                        wait = backoff.first;
                        continue;
                    }
                    Some(Ok(Err(error))) => {
                        unanswered = true;
                        hang_up().await.unwrap_or_else(|| error.to_string())
                    }
                }
            }
        };
        // A replace-if-idle that found the engine busy by the time it
        // asked: left running, with the restart offered. One that had let
        // the app in is dialled again at once and used, as a busy one found
        // first is; `replaced` stays, so it is left running.
        let seen = replacing.take();
        if let (Some(busy), Some(seen)) = (provision::busy_line(&why), seen.as_ref()) {
            tracing::info!(host, ?busy, "the engine was busy; not replacing it");
            if seen.welcomed {
                if !link.say(EngineLink::Connecting { host: host.clone() }) {
                    return;
                }
                continue;
            }
            let other = OtherEngine {
                runs: busy.runs,
                clients: busy.clients,
                ..seen.other.clone()
            };
            let why = "Perch would have restarted it to run its own, but it is in use.".to_string();
            if !link.halt(why, ever, Some(other)).await {
                return;
            }
            (replace, replaced) = (link.after_wake(), false);
            wait = backoff.first;
            continue;
        }
        let installing = |build: &str, progress| {
            link.say(EngineLink::Installing {
                host: host.clone(),
                build: build.to_string(),
                progress,
            });
        };
        let why = match install_if_asked(install.as_mut(), &host, &why, just_installed, installing)
            .await
        {
            None => why,
            Some(Ok(build)) => {
                // Tried again at once: nothing failed. A replace or restart
                // the try was for is still asked.
                installed = Some(build);
                (replace, replacing) = (asked, seen);
                if !link.say(EngineLink::Connecting { host: host.clone() }) {
                    return;
                }
                continue;
            }
            Some(Err(Failed { why, lasting: true })) => {
                tracing::warn!(host, %why, "perchd could not be put on the host");
                if !link.halt(why, ever, None).await {
                    return;
                }
                (replace, replaced) = (link.after_wake(), false);
                wait = backoff.first;
                continue;
            }
            Some(Err(Failed { why, .. })) => why,
        };
        tracing::warn!(host, %why, ?wait, "not connected to the engine");
        link.down();
        if !link.say(EngineLink::Down {
            host: host.clone(),
            why,
            retry_in: Some(wait),
            ever,
            other: None,
        }) {
            return;
        }
        let asked_now = tokio::select! {
            () = tokio::time::sleep(wait) => false,
            () = link.retry.notified() => true,
            () = link.restart.notified() => true,
        };
        if asked_now {
            (replace, replaced) = (link.after_wake(), false);
        } else if unanswered {
            (replace, replacing) = (asked, seen);
        }
        wait = backoff.after(wait);
    }
}

/// What a try does with the engine it reached.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Judged {
    /// Use it: the app's build, or any engine, for a `[remote] command`,
    /// which wants none.
    Use,
    /// Another build with no runs and no other client, and this connect
    /// has not replaced one yet: dial again, replacing it if still idle.
    Replace(OtherEngine),
    /// Leave it running and say so: another build that is busy, or one
    /// already replaced once this connect.
    Leave(OtherEngine),
}

/// An automatic replace under way: the engine it is for, and whether
/// that engine let the app in, speaking its protocol.
#[derive(Debug, Clone)]
struct Replacing {
    other: OtherEngine,
    welcomed: bool,
}

/// An engine's build as the windows say it: its version, and the build
/// when that is a hash, which says which of a version's it is. One from
/// before builds were hashes names its version in its build already.
pub fn engine_said(version: &str, build: &str) -> String {
    if build.is_empty() || build == version {
        version.to_string()
    } else if build.starts_with(version) {
        build.to_string()
    } else {
        format!("{version} ({build})")
    }
}

/// The app's own build as the windows say it, of `wanted`: its version,
/// and the build when there is only one it could be.
fn wanted_said(wanted: &[String]) -> String {
    match wanted {
        [build] => engine_said(VERSION, build),
        _ => VERSION.to_string(),
    }
}

/// Weigh the engine `about` describes against `wanted`, the app's builds.
/// `may_replace` while this connect has not replaced one. Builds are the
/// same or they are not: neither is the newer, and an engine that is not
/// the app's makes way for it once it has nothing running.
fn judge(wanted: Option<&[String]>, about: &super::About, may_replace: bool) -> Judged {
    let Some(wanted) = wanted else {
        return Judged::Use;
    };
    if wanted.contains(&about.build) {
        return Judged::Use;
    }
    let other = OtherEngine {
        engine: engine_said(&about.engine_version, &about.build),
        wanted: wanted_said(wanted),
        runs: about.runs,
        clients: about.clients,
    };
    if may_replace && about.runs == 0 && about.clients == 0 {
        Judged::Replace(other)
    } else {
        Judged::Leave(other)
    }
}

/// What the windows say of an engine of another build left running.
pub fn other_said(host: &str, other: &OtherEngine) -> String {
    let OtherEngine {
        engine,
        wanted,
        runs,
        clients,
    } = other;
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let cost = if *runs > 0 {
        format!(" ({runs} agent run{} would be stopped)", plural(*runs))
    } else if *clients > 0 {
        format!(
            " ({clients} other client{} would be disconnected)",
            plural(*clients)
        )
    } else {
        String::new()
    };
    format!(
        "{host}'s engine is perchd {engine}; restart it to run this app's, perchd {wanted}{cost}."
    )
}

/// When `why`, the last thing a try said, is the host saying it has no
/// `perchd` of the app's build: send it, saying so with `installing`, and
/// give what was sent. None for any other end, or with nothing to send
/// with. A try right after a send that says so again is not sent for
/// again: the host does not find what was put there, and sending it again
/// would find it no better.
async fn install_if_asked(
    install: Option<&mut Install>,
    host: &str,
    why: &str,
    just_installed: Option<String>,
    mut installing: impl FnMut(&str, Option<crate::backend::InstallProgress>),
) -> Option<Result<String, Failed>> {
    let install = install?;
    let asked = provision::install_line(why)?;
    Some(match (asked, just_installed) {
        (Err(why), _) => Err(Failed::lasting(why)),
        (Ok(_), Some(build)) => Err(Failed::lasting(format!(
            "Perch put perchd {build} on {host}, and the host still says it has none. Check that a login shell there sees the same $PERCH_HOME, or set [remote] command to run perchd yourself."
        ))),
        (Ok(arch), None) => {
            let (progress, mut updates) = tokio::sync::watch::channel(None);
            let (build, mut installed) = install(arch, progress);
            tracing::info!(host, build, platform = arch.name(), "sending perchd");
            installing(&build, None);
            loop {
                tokio::select! {
                    result = &mut installed => break result.map(|()| {
                        tracing::info!(host, build, "sent perchd");
                        build
                    }),
                    Ok(()) = updates.changed() => {
                        let progress = updates.borrow_and_update().clone();
                        installing(&build, progress);
                    }
                }
            }
        }
    })
}

/// Connect to the engine on `host` once, sending `perchd` first if it is
/// missing, and replacing one of another build with nothing running, and give
/// its welcome, with the engine of another build left running if it is
/// one, and the host's checks, or why there was none: what `perch
/// --connect` does. `said` hears what is done on the way: the build being
/// installed, or the engine being replaced.
pub async fn connect_once(
    host: &str,
    bridge: Bridge,
    answer: Duration,
    mut said: impl FnMut(&str),
) -> Result<Connected, String> {
    let (mut dial, mut install) = dial_for(host, bridge);
    let mut installed: Option<String> = None;
    let mut replace = Replace::No;
    let mut replacing: Option<Replacing> = None;
    loop {
        let just_installed = installed.take();
        let asked = std::mem::take(&mut replace);
        let Dialed {
            stream,
            hang_up,
            build: wanted,
        } = dial(asked).await?;
        let may_replace = replacing.is_none();
        let within = answer_wait(answer, asked);
        let why = match tokio::time::timeout(within, handshake(stream)).await {
            Ok(Ok((welcome, session))) => {
                let judged = judge(wanted.as_deref(), &welcome.about(), may_replace);
                let (other, checks) = match judged {
                    Judged::Replace(other) => {
                        drop(session);
                        let _ = hang_up().await;
                        said(&format!(
                            "Restarting {host}'s engine, perchd {}, to run perchd {}…",
                            other.engine, other.wanted
                        ));
                        replace = Replace::IfIdle;
                        replacing = Some(Replacing {
                            other,
                            welcomed: true,
                        });
                        continue;
                    }
                    Judged::Leave(other) => (Some(other), session.host_checks(HOST_CHECKS).await),
                    Judged::Use => (None, session.host_checks(HOST_CHECKS).await),
                };
                let _ = hang_up().await;
                return Ok(Connected {
                    welcome,
                    other,
                    checks,
                });
            }
            Ok(Err(WireError::Refused(reason, about))) => {
                let _ = hang_up().await;
                match judge(wanted.as_deref(), &about, may_replace) {
                    Judged::Replace(other) if about.protocol != 0 => {
                        said(&format!(
                            "Restarting {host}'s engine, perchd {}, to run perchd {}…",
                            other.engine, other.wanted
                        ));
                        replace = Replace::IfIdle;
                        replacing = Some(Replacing {
                            other,
                            welcomed: false,
                        });
                        continue;
                    }
                    Judged::Leave(other) if about.protocol != 0 => {
                        return Err(format!("{} {reason}", other_said(host, &other)))
                    }
                    _ => return Err(reason),
                }
            }
            Ok(Err(error)) => hang_up().await.unwrap_or_else(|| error.to_string()),
            Err(_) => hang_up()
                .await
                .unwrap_or_else(|| format!("{host} did not answer within {} s.", within.as_secs())),
        };
        if let (Some(busy), Some(seen)) = (provision::busy_line(&why), replacing.as_ref()) {
            // One that let the app in is used as it is, left running.
            if seen.welcomed {
                said(&format!(
                    "{host}'s engine came into use before it could be restarted; connecting to it as it is…"
                ));
                continue;
            }
            let other = OtherEngine {
                runs: busy.runs,
                clients: busy.clients,
                ..seen.other.clone()
            };
            return Err(format!(
                "{} It came into use before it could be restarted.",
                other_said(host, &other)
            ));
        }
        let mut installing = |build: &str, progress: Option<crate::backend::InstallProgress>| {
            if progress.is_none() {
                said(&format!("Sending perchd {build} to {host}…"));
            }
        };
        match install_if_asked(
            install.as_mut(),
            host,
            &why,
            just_installed,
            &mut installing,
        )
        .await
        {
            None => return Err(why),
            // A replace the try was for is still asked.
            Some(Ok(build)) => (installed, replace) = (Some(build), asked),
            Some(Err(failed)) => return Err(failed.why),
        }
    }
}

/// How long `connect_once` waits for the host's checks once connected. An
/// engine a bridge has just started looks at the host after it has found
/// the agents, which can take a few seconds.
const HOST_CHECKS: Duration = Duration::from_secs(30);

/// What `connect_once` connected to.
#[derive(Debug)]
pub struct Connected {
    pub welcome: super::Welcome,
    /// The engine of another build left running, when it is one.
    pub other: Option<OtherEngine>,
    /// What the host lacks, or `None` when the engine did not say in time.
    pub checks: Option<Vec<crate::host::HostCheck>>,
}

/// Why `host` cannot be handed to ssh, or None when it can: ssh would read
/// one that starts with a dash as an option.
pub fn bad_host(host: &str) -> Option<String> {
    if host.trim().is_empty() {
        Some("No host is named.".to_string())
    } else if host.starts_with('-') {
        Some(format!("{host} is not a host ssh can be given."))
    } else if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        Some(format!(
            "\"{host}\" has a space in it, which no host name does."
        ))
    } else {
        // What ssh would be handed, past a scheme and a port: a user
        // with no host after it, or brackets with nothing in them, names
        // none, and a name that starts with a dash is an option to ssh.
        let (name, _) = host_and_port(host);
        let machine = name.rsplit_once('@').map_or(name.as_str(), |(_, m)| m);
        if machine.is_empty() || name.contains("://") {
            Some(format!("{host} names no host."))
        } else if name.starts_with('-') {
            Some(format!("{host} is not a host ssh can be given."))
        } else {
            None
        }
    }
}

/// `text` as one word to a POSIX shell.
pub fn shell_quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@%+=:,./_-".contains(&b))
    {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// What ssh runs on the host: `command` in the reader's login shell, so
/// what their profile puts on `PATH`, where `perchd` was installed, is on
/// it. ssh on its own runs a command in a shell that reads no profile.
pub fn in_login_shell(command: &str) -> String {
    format!("exec \"$SHELL\" -lc {}", shell_quote(command))
}

/// ssh's arguments to run `remote`, a command line for the shell ssh
/// starts there, on `host`: no terminal, so nothing but the wire is on its
/// stdout, and a check every 15 s that the host is still there, given up
/// after three go unanswered.
///
/// Whatever the reader's ssh config says, the agent and X11 are not
/// forwarded and no port forwardings are made: the engine outlives this
/// session and would hand a forwarded agent to every run it starts, and
/// a forwarding made again on each reconnect can collide with the last.
/// Nothing is asked of the reader either, as there is no terminal to ask
/// in: a host whose key ssh has not seen, or a key with no agent to
/// unlock it, fails the try with ssh's reason.
pub fn ssh_args(host: &str, remote: &str) -> Vec<String> {
    let (host, port) = host_and_port(host);
    let port = port.map(|port| port.to_string());
    port.iter()
        .flat_map(|port| ["-p", port.as_str()])
        .chain(SSH_OPTIONS)
        .chain(["--", host.as_str(), remote])
        .map(str::to_string)
        .collect()
}

/// A host as it is named in `[remote] host`, apart from the port after
/// it: `chris@halo:2222` is `chris@halo` and 2222. ssh takes a port only
/// as an option, so everything that hands ssh the host goes through here.
/// An IPv6 address has a port only in brackets, `[::1]:2222`, since its
/// own colons could be read as one; ssh is given it without them.
pub fn host_and_port(host: &str) -> (String, Option<u16>) {
    if let Some((before, after)) = host.rsplit_once(']') {
        if let Some((user, address)) = before.split_once('[') {
            let port = after.strip_prefix(':').and_then(|port| port.parse().ok());
            if after.is_empty() || port.is_some() {
                return (format!("{user}{address}"), port);
            }
        }
        return (host.to_string(), None);
    }
    match host.rsplit_once(':') {
        Some((name, port)) if !name.is_empty() && !name.contains(':') => match port.parse() {
            Ok(port) => (name.to_string(), Some(port)),
            Err(_) => (host.to_string(), None),
        },
        _ => (host.to_string(), None),
    }
}

/// The options every ssh the app starts for the engine is given.
const SSH_OPTIONS: [&str; 15] = [
    "-T",
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=10",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=3",
    "-o",
    "ForwardAgent=no",
    "-o",
    "ForwardX11=no",
    "-o",
    "ClearAllForwardings=yes",
];

/// The remote line ssh runs for `bridge`: its command, or the bridge of
/// whichever of `builds` is the host's, replacing an engine of another
/// build as `replace` says.
fn remote_line(bridge: &Bridge, builds: &[(String, String)], replace: Replace) -> String {
    match bridge {
        Bridge::Command(command) => in_login_shell(command),
        Bridge::Installed(_) => provision::connect_command(builds, replace),
    }
}

/// Dial `host` with ssh (`PERCH_SSH` when set), which runs `remote` there.
/// ssh's stderr goes to the log, and its last line is why a connection
/// ended. A host saying it has no `perchd` to run, or a bridge saying the
/// engine is busy, keeps that as the last line, whatever ssh says after it.
async fn dial_ssh(host: &str, remote: &str, build: Option<Wanted>) -> Result<Dialed, String> {
    if let Some(why) = bad_host(host) {
        return Err(why);
    }
    let mut child = tokio::process::Command::new(provision::ssh_program())
        .args(ssh_args(host, remote))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Could not run ssh: {error}"))?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err("ssh started with no pipes to it".to_string());
    };
    let last = Arc::new(Mutex::new(None::<String>));
    let said = tokio::spawn({
        let (last, host) = (last.clone(), host.to_string());
        async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                tracing::info!(host, "ssh: {line}");
                if let Ok(mut last) = last.lock() {
                    let kept = last.as_deref().is_some_and(|was| {
                        was.starts_with(provision::INSTALL_SAID)
                            || was.starts_with(provision::BUSY_SAID)
                    });
                    if !kept {
                        *last = Some(line.to_string());
                    }
                }
            }
        }
    });
    Ok(Dialed {
        stream: Box::new(tokio::io::join(stdout, stdin)),
        build,
        hang_up: Box::new(move || {
            Box::pin(async move {
                // Most often ssh has gone already, or is going, and its
                // last words are on their way.
                if tokio::time::timeout(Duration::from_secs(2), child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.kill().await;
                }
                // A master connection ssh shares (ControlPersist) keeps its
                // stderr open past it: the reader is stopped rather than
                // left with the pipe.
                let mut said = said;
                if tokio::time::timeout(Duration::from_secs(1), &mut said)
                    .await
                    .is_err()
                {
                    said.abort();
                }
                last.lock().ok().and_then(|last| last.clone())
            })
        }),
    })
}

/// What a reader knows the `perchd` an app run from source built as.
const DEV_BINARY: &str = "The perchd built from source";

/// How to reach the engine on `host` through `bridge`, and, with no
/// command given, how to send `perchd` there. The app names the build of
/// each engine it carries, and the host runs the one for what it is. For
/// a host it carries none for, it builds one from its source once the
/// host has said what it is, and names that from then on: its build is
/// worked out for each try, so one built again while the app runs is
/// sent on the next.
pub fn dial_for(host: &str, bridge: Bridge) -> (Dial, Option<Install>) {
    // The `perchd` built from source for this host, once one has been:
    // the dial can name its build only then. Set when a build succeeds
    // and not before, so a try after a failed one builds again rather
    // than naming whatever an earlier session left on disk.
    let built: Arc<Mutex<Option<PathBuf>>> = Arc::default();
    let dial: Dial = Box::new({
        let (host, bridge, built) = (host.to_string(), bridge.clone(), built.clone());
        move |replace| {
            let (host, bridge) = (host.clone(), bridge.clone());
            let built = built.lock().ok().and_then(|built| built.clone());
            Box::pin(async move {
                let builds = match &bridge {
                    Bridge::Command(_) => Vec::new(),
                    Bridge::Installed(source) => {
                        // One for each platform the app carries an engine
                        // for: the host runs its own, or says what it is.
                        let mut builds: Vec<(String, String)> = source
                            .carried
                            .iter()
                            .map(|engine| (engine.platform.key(), engine.build().to_string()))
                            .collect();
                        // And the one built from source for this host,
                        // which carries none for it: the last named wins.
                        if let Some(binary) = built.filter(|binary| binary.is_file()) {
                            let binary = tokio::task::spawn_blocking(move || {
                                provision::read_binary(&binary, DEV_BINARY)
                            })
                            .await
                            .map_err(|e| e.to_string())??;
                            builds.push((provision::ANY.to_string(), binary.build));
                        }
                        builds
                    }
                };
                let remote = remote_line(&bridge, &builds, replace);
                let wanted = matches!(bridge, Bridge::Installed(_))
                    .then(|| builds.into_iter().map(|(_, build)| build).collect());
                dial_ssh(&host, &remote, wanted).await
            })
        }
    });
    let source = match bridge {
        Bridge::Command(_) => return (dial, None),
        Bridge::Installed(source) => source,
    };
    let host = host.to_string();
    let install: Install = Box::new(move |platform, progress| {
        let host = host.clone();
        // The one the app carries for what the host said it is. The host
        // has just said it has no such build.
        if let Some(engine) = source.carried_for(platform).cloned() {
            return (
                VERSION.to_string(),
                Box::pin(async move {
                    provision::phase(&progress, "Unpacking perchd");
                    let binary = tokio::task::spawn_blocking(move || engine.binary())
                        .await
                        .map_err(|e| Failed::now(e.to_string()))??;
                    provision::upload(&host, binary, &progress).await
                }),
            );
        }
        let Some(workspace) = source.workspace.clone() else {
            return (
                VERSION.to_string(),
                Box::pin(async move { Err(Failed::lasting(provision::none_carried(platform))) }),
            );
        };
        // None carried for it: built now, for what the host said it is,
        // from the source as it stands; uploaded unless the host has
        // that very build. Forgotten until this build is done: one that
        // fails leaves the next try to build again.
        if let Ok(mut built) = built.lock() {
            *built = None;
        }
        let built = built.clone();
        (
            format!("{VERSION} (built from source)"),
            Box::pin(async move {
                provision::phase(&progress, "Building perchd from source");
                let path = provision::dev_build(&workspace, platform).await?;
                let read = path.clone();
                let binary =
                    tokio::task::spawn_blocking(move || provision::read_binary(&read, DEV_BINARY))
                        .await
                        .map_err(|e| Failed::now(e.to_string()))?
                        .map_err(Failed::lasting)?;
                if !provision::has_build(&host, &binary.build).await {
                    provision::upload(&host, binary, &progress).await?;
                }
                if let Ok(mut built) = built.lock() {
                    *built = Some(path);
                }
                Ok(())
            }),
        )
    });
    (dial, Some(install))
}

/// The app's connection to the engine on `host`, through `bridge`, carried
/// by a runtime of the client's own. With no runtime to be had, a handle
/// with nothing behind it, and a status that says why.
pub fn over_ssh(
    host: &str,
    bridge: Bridge,
) -> (Backend, std::sync::mpsc::Receiver<(WindowId, Event)>) {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let runtime = match RUNTIME.get() {
        Some(runtime) => runtime,
        None => match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("perch-link")
            .enable_all()
            .build()
        {
            Ok(built) => RUNTIME.get_or_init(|| built),
            Err(error) => {
                tracing::error!(%error, "could not start the runtime the link runs on");
                let (events, received) = std::sync::mpsc::channel();
                let _ = events.send((
                    NO_WINDOW,
                    Event::Status(crate::backend::Status::Error {
                        message: format!("Perch could not connect to {host}: {error}."),
                    }),
                ));
                let (backend, _) = Backend::over_wire(tokio::sync::mpsc::unbounded_channel().0, 0);
                return (backend, received);
            }
        },
    };
    // What an app from before engines were carried downloaded here.
    runtime.spawn_blocking(|| {
        provision::clear_old_downloads(&perch_github::config::perch_home().join("perchd"))
    });
    let (dial, install) = dial_for(host, bridge);
    let (link, received) = start(host, dial, install, Backoff::default(), runtime.handle());
    // This app's own updates, looked for here: the engine on the host
    // looks for none.
    runtime.spawn(update::clock(
        link.releases.clone(),
        EventSender::routed(link.events.clone()),
    ));
    (Backend::linked(link), received)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{encode, next_message, About, Answer, CommandLine, Hello, Welcome, PROTOCOL};
    use tokio::io::{AsyncWriteExt, BufReader, DuplexStream};

    #[test]
    fn the_wait_doubles_from_a_second_to_half_a_minute() {
        let backoff = Backoff::default();
        let mut wait = backoff.first;
        let mut waits = vec![wait];
        for _ in 0..6 {
            wait = backoff.after(wait);
            waits.push(wait);
        }
        assert_eq!(
            waits.iter().map(Duration::as_secs).collect::<Vec<_>>(),
            [1, 2, 4, 8, 16, 30, 30]
        );
    }

    #[test]
    fn a_port_after_the_host_is_ssh_s_option() {
        let split = |host: &str| {
            let (name, port) = host_and_port(host);
            (name, port)
        };
        assert_eq!(split("halo"), ("halo".into(), None));
        assert_eq!(split("chris@halo"), ("chris@halo".into(), None));
        assert_eq!(
            split("chris@halo:12354"),
            ("chris@halo".into(), Some(12354))
        );
        assert_eq!(split("10.0.0.5:22"), ("10.0.0.5".into(), Some(22)));
        // An IPv6 address's own colons are not a port; in brackets one
        // can follow.
        assert_eq!(split("fe80::1"), ("fe80::1".into(), None));
        assert_eq!(split("[fe80::1]"), ("fe80::1".into(), None));
        assert_eq!(split("[fe80::1]:2222"), ("fe80::1".into(), Some(2222)));
        assert_eq!(
            split("chris@[fe80::1]:2222"),
            ("chris@fe80::1".into(), Some(2222))
        );
        // Not a port: left for ssh to refuse.
        assert_eq!(split("halo:ssh"), ("halo:ssh".into(), None));
        assert_eq!(split("halo:99999"), ("halo:99999".into(), None));

        let args = ssh_args("chris@halo:12354", "true");
        assert_eq!(&args[..3], ["-p", "12354", "-T"]);
        assert_eq!(&args[args.len() - 3..], ["--", "chris@halo", "true"]);
        assert_eq!(ssh_args("halo", "true")[0], "-T");
    }

    #[test]
    fn ssh_is_given_the_host_and_a_command_for_the_login_shell() {
        assert_eq!(
            ssh_args("halo", &in_login_shell("perchd bridge")),
            [
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ForwardX11=no",
                "-o",
                "ClearAllForwardings=yes",
                "--",
                "halo",
                "exec \"$SHELL\" -lc 'perchd bridge'",
            ]
        );
        assert_eq!(
            in_login_shell("~/bin/perchd bridge --it's"),
            r#"exec "$SHELL" -lc '~/bin/perchd bridge --it'\''s'"#
        );
        assert_eq!(shell_quote("/srv/code/perch"), "/srv/code/perch");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b$c"), "'a b$c'");
        assert!(bad_host("-oProxyCommand=x").is_some());
        assert!(bad_host("halo box").is_some());
        assert!(bad_host(" ").is_some());
        assert_eq!(bad_host("chris@halo.local"), None);
        assert_eq!(bad_host("chris@halo:2222"), None);
        assert_eq!(bad_host("[fe80::1]:2222"), None);
        for none in ["[]:2222", "chris@", "ssh://halo", "chris@:22"] {
            assert!(bad_host(none).is_some(), "{none}");
        }
    }

    /// The engine's end of a stream, played by the test.
    struct Far {
        read: BufReader<tokio::io::ReadHalf<DuplexStream>>,
        write: tokio::io::WriteHalf<DuplexStream>,
        line: Vec<u8>,
    }

    impl Far {
        async fn hello(&mut self) -> Hello {
            next_message::<_, Hello>(&mut self.read, &mut self.line)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        }

        async fn answer(&mut self, answer: Answer) {
            self.write
                .write_all(&encode(&answer).unwrap())
                .await
                .unwrap();
        }

        async fn welcome(&mut self, connection: u64) {
            self.welcome_as(connection, &engine(VERSION, VERSION, 0, 0))
                .await;
        }

        /// Let the client in as the engine `about` says.
        async fn welcome_as(&mut self, connection: u64, about: &About) {
            self.hello().await;
            self.answer(Answer::Welcome(Welcome {
                protocol: PROTOCOL,
                engine_version: about.engine_version.clone(),
                connection,
                host: "halo".into(),
                home: "/home/chris".into(),
                data_root: "/home/chris/.perch".into(),
                clients: about.clients,
                runs: about.runs,
                build: about.build.clone(),
            }))
            .await;
        }

        /// The next command, or None once the client has hung up.
        async fn command(&mut self) -> Option<(WindowId, Command)> {
            let next = tokio::time::timeout(
                Duration::from_secs(5),
                next_message::<_, CommandLine>(&mut self.read, &mut self.line),
            )
            .await
            .expect("a command in time")
            .unwrap()?;
            let CommandLine { window, command } = next.unwrap();
            Some((window, command))
        }

        /// Whether the client sends anything in `within`.
        async fn quiet(&mut self, within: Duration) -> bool {
            tokio::time::timeout(
                within,
                next_message::<_, CommandLine>(&mut self.read, &mut self.line),
            )
            .await
            .is_err()
        }
    }

    /// An engine as it says it is.
    fn engine(version: &str, build: &str, runs: usize, clients: usize) -> About {
        About {
            engine_version: version.into(),
            protocol: PROTOCOL,
            build: build.into(),
            runs,
            clients,
        }
    }

    /// A dial that hands the client one end of a pipe in memory and the
    /// test the other, or fails with `why` while `refusing` holds it.
    struct Pipes {
        far: tokio::sync::mpsc::UnboundedReceiver<Far>,
        dials: Arc<std::sync::atomic::AtomicUsize>,
        failing: Arc<Mutex<Option<String>>>,
        /// What a dial says last as it is hung up, ssh's last line.
        last: Arc<Mutex<Option<String>>>,
        /// How each dial was asked to treat another build.
        asked: Arc<Mutex<Vec<Replace>>>,
        /// How long a dial takes before it has a stream, as one that has
        /// the host ready its `perchd` first.
        stall: Arc<Mutex<Duration>>,
    }

    /// Dials as a `[remote] command`, which wants no build.
    fn pipes() -> (Dial, Pipes) {
        pipes_for(None)
    }

    /// Dials of the `perchd` of `build`, the app's own.
    fn pipes_for(build: Option<&str>) -> (Dial, Pipes) {
        let build = build.map(|build| vec![build.to_string()]);
        let (to_test, far) = tokio::sync::mpsc::unbounded_channel();
        let dials = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failing = Arc::new(Mutex::new(None::<String>));
        let last = Arc::new(Mutex::new(None::<String>));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let stall = Arc::new(Mutex::new(Duration::ZERO));
        let dial: Dial = Box::new({
            let (dials, failing, last, asked, stall) = (
                dials.clone(),
                failing.clone(),
                last.clone(),
                asked.clone(),
                stall.clone(),
            );
            move |replace| {
                dials.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                asked.lock().unwrap().push(replace);
                let failing = failing.lock().unwrap().clone();
                let said = last.lock().unwrap().clone();
                let to_test = to_test.clone();
                let build = build.clone();
                let stall = *stall.lock().unwrap();
                Box::pin(async move {
                    tokio::time::sleep(stall).await;
                    if let Some(why) = failing {
                        return Err(why);
                    }
                    let (near, far) = tokio::io::duplex(1 << 16);
                    let (read, write) = tokio::io::split(far);
                    let _ = to_test.send(Far {
                        read: BufReader::new(read),
                        write,
                        line: Vec::new(),
                    });
                    Ok(Dialed {
                        stream: Box::new(near),
                        build,
                        hang_up: Box::new(move || Box::pin(async move { said })),
                    })
                })
            }
        });
        (
            dial,
            Pipes {
                far,
                dials,
                failing,
                last,
                asked,
                stall,
            },
        )
    }

    impl Pipes {
        async fn next(&mut self) -> Far {
            tokio::time::timeout(Duration::from_secs(5), self.far.recv())
                .await
                .expect("a dial in time")
                .unwrap()
        }

        fn dials(&self) -> usize {
            self.dials.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    const QUICK: Backoff = Backoff {
        first: Duration::from_millis(20),
        last: Duration::from_millis(80),
        answer: Duration::from_secs(10),
        steady: Duration::from_secs(10),
    };

    /// The next event the windows would get, waiting for it.
    async fn next_event(
        events: &std::sync::mpsc::Receiver<(WindowId, Event)>,
    ) -> (WindowId, Event) {
        for _ in 0..1000 {
            if let Ok(event) = events.try_recv() {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("no event in time");
    }

    async fn next_link(events: &std::sync::mpsc::Receiver<(WindowId, Event)>) -> EngineLink {
        loop {
            if let (_, Event::Link(link)) = next_event(events).await {
                return link;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_command_while_disconnected_is_refused_with_a_notice_and_never_sent() {
        let (dial, mut pipes) = pipes();
        *pipes.failing.lock().unwrap() = Some("ssh: Could not resolve hostname halo".into());
        let (backend, events) = link(
            "halo",
            dial,
            None,
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        assert_eq!(
            next_link(&events).await,
            EngineLink::Connecting {
                host: "halo".into()
            }
        );
        // ssh's last word is the reason given.
        let EngineLink::Down {
            why,
            retry_in,
            ever,
            ..
        } = next_link(&events).await
        else {
            panic!("not down");
        };
        assert_eq!(why, "ssh: Could not resolve hostname halo");
        assert_eq!(retry_in, Some(QUICK.first));
        assert!(!ever);

        let window = backend.open_window();
        window.refresh_open_item("item");
        loop {
            match next_event(&events).await {
                (to, Event::Notice(message)) => {
                    assert_eq!(to, window.window());
                    assert_eq!(message, refused("halo"));
                    break;
                }
                (_, Event::Link(_)) => {}
                other => panic!("{other:?}"),
            }
        }
        // A second refusal straight after is not said again.
        window.refresh_open_item("item");

        *pipes.failing.lock().unwrap() = None;
        let mut far = pipes.next().await;
        far.welcome(3).await;
        // The window is opened, then everything asked for; the refused
        // command never goes.
        assert_eq!(
            far.command().await.map(|(w, c)| (w, format!("{c:?}"))),
            Some((window.window(), "WindowOpened".to_string()))
        );
        assert!(matches!(
            far.command().await,
            Some((w, Command::WindowKey { key: true })) if w == window.window()
        ));
        assert!(matches!(
            far.command().await,
            Some((NO_WINDOW, Command::Resync))
        ));
        assert!(far.quiet(Duration::from_millis(100)).await);
        loop {
            match next_event(&events).await {
                (_, Event::Link(EngineLink::Up { again, welcome, .. })) => {
                    // A command was refused on the way, so the windows
                    // open again what they had.
                    assert!(again);
                    assert_eq!(welcome.home, std::path::PathBuf::from("/home/chris"));
                    break;
                }
                (_, Event::Link(_)) => {}
                (_, Event::Notice(message)) => panic!("said again: {message}"),
                other => panic!("{other:?}"),
            }
        }
        // Connected, a command goes.
        window.refresh_open_item("item");
        assert!(matches!(
            far.command().await,
            Some((_, Command::RefreshOpenItem { .. }))
        ));
    }

    /// What `window` hears of its update commands, ignoring the link.
    async fn update_answer(
        events: &std::sync::mpsc::Receiver<(WindowId, Event)>,
        window: WindowId,
    ) -> Event {
        loop {
            match next_event(events).await {
                (_, Event::Link(_)) => {}
                (to, event @ (Event::UpdateChecked(_) | Event::Update(_))) => {
                    assert_eq!(to, window);
                    return event;
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_apps_own_updates_are_looked_for_here_and_never_sent_to_the_host() {
        use crate::update::{Checked, UpdateStatus};
        let (dial, mut pipes) = pipes();
        *pipes.failing.lock().unwrap() = Some("ssh: connect to host halo: refused".into());
        let (backend, events) = link(
            "halo",
            dial,
            None,
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        while !matches!(next_link(&events).await, EngineLink::Down { .. }) {}
        let window = backend.open_window();

        // Not connected, the look is still made, here, and not refused. A
        // test runs from no app, which is what it finds.
        window.check_for_update();
        assert!(matches!(
            update_answer(&events, window.window()).await,
            Event::UpdateChecked(Checked::NotARelease)
        ));

        *pipes.failing.lock().unwrap() = None;
        let mut far = pipes.next().await;
        far.welcome(1).await;
        for _ in 0..3 {
            far.command().await.unwrap();
        }
        while !matches!(next_link(&events).await, EngineLink::Up { .. }) {}

        // Connected, neither goes to the engine.
        window.check_for_update();
        assert!(matches!(
            update_answer(&events, window.window()).await,
            Event::UpdateChecked(Checked::NotARelease)
        ));
        window.install_update("9.9.9");
        let Event::Update(failed) = update_answer(&events, window.window()).await else {
            panic!("not an update");
        };
        assert_eq!(
            failed,
            UpdateStatus::Failed {
                version: "9.9.9".into(),
                error: "Perch isn't running from an app it can replace.".into(),
            }
        );
        assert!(far.quiet(Duration::from_millis(200)).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_lost_connection_is_made_again_and_the_windows_resync() {
        let (dial, mut pipes) = pipes();
        let (backend, events) = link(
            "halo",
            dial,
            None,
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        let (first, second) = (backend.open_window(), backend.open_window());
        // Asked for as the app starts, before the first try is through:
        // it goes once the windows are open and everything asked for.
        first.refresh_open_item("item");
        let mut far = pipes.next().await;
        far.welcome(1).await;
        let mut opened = Vec::new();
        for _ in 0..6 {
            opened.push(far.command().await.unwrap());
        }
        assert!(matches!(opened[4], (NO_WINDOW, Command::Resync)));
        assert!(matches!(opened[5], (w, Command::RefreshOpenItem { .. }) if w == first.window()));
        second.close_window();
        assert!(matches!(
            far.command().await,
            Some((w, Command::WindowClosed)) if w == second.window()
        ));

        // The engine goes.
        drop(far);
        loop {
            if let EngineLink::Down { ever, retry_in, .. } = next_link(&events).await {
                assert!(ever);
                assert_eq!(retry_in, Some(QUICK.first));
                break;
            }
        }
        let mut far = pipes.next().await;
        far.welcome(2).await;
        // Only the window still open is opened again, then the resync.
        assert!(matches!(
            far.command().await,
            Some((w, Command::WindowOpened)) if w == first.window()
        ));
        assert!(matches!(
            far.command().await,
            Some((w, Command::WindowKey { .. })) if w == first.window()
        ));
        assert!(matches!(
            far.command().await,
            Some((NO_WINDOW, Command::Resync))
        ));
        loop {
            if let EngineLink::Up { again, .. } = next_link(&events).await {
                assert!(again);
                break;
            }
        }
        // A scope pick goes, numbered by the window as before.
        first.set_scope("item", perch_git::Scope::Branch);
        assert!(matches!(
            far.command().await,
            Some((_, Command::SetScope { scope_id, .. })) if scope_id > 0
        ));
    }

    /// A window that loses or gains the keyboard while disconnected is not
    /// told off for it, and the engine hears where the keyboard is once
    /// connected again: it takes every window it opens as key.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn which_window_is_key_is_kept_through_a_lost_connection() {
        let (dial, mut pipes) = pipes();
        let (backend, events) = link(
            "halo",
            dial,
            None,
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        let (first, second) = (backend.open_window(), backend.open_window());
        let mut far = pipes.next().await;
        far.welcome(1).await;
        for _ in 0..5 {
            far.command().await.unwrap();
        }
        drop(far);
        loop {
            if let EngineLink::Down { .. } = next_link(&events).await {
                break;
            }
        }
        // The app goes to the background while the link is down.
        first.set_window_key(false);
        second.set_window_key(false);
        let mut far = pipes.next().await;
        far.welcome(2).await;
        let mut sent = Vec::new();
        for _ in 0..5 {
            sent.push(far.command().await.unwrap());
        }
        for window in [first.window(), second.window()] {
            assert!(sent
                .iter()
                .any(|(w, c)| *w == window && matches!(c, Command::WindowKey { key: false })));
        }
        loop {
            match next_event(&events).await {
                (_, Event::Link(EngineLink::Up { .. })) => break,
                (_, Event::Notice(message)) => panic!("told off: {message}"),
                _ => {}
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn what_the_windows_asked_for_as_a_failed_first_try_ran_is_said_dropped() {
        let (dial, mut pipes) = pipes();
        let (backend, events) = link(
            "halo",
            dial,
            None,
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        let (first, second) = (backend.open_window(), backend.open_window());
        first.refresh_open_item("item");
        // The first try is under way, and fails.
        let far = pipes.next().await;
        drop(far);
        let mut told = Vec::new();
        loop {
            match next_event(&events).await {
                (to, Event::Notice(message)) => {
                    assert_eq!(message, refused("halo"));
                    told.push(to);
                }
                (_, Event::Link(EngineLink::Down { .. })) => break,
                (_, Event::Link(_)) => {}
                other => panic!("{other:?}"),
            }
        }
        // Only the window that asked.
        assert_eq!(told, [first.window()]);
        // Refusals are said to each window on its own clock.
        first.refresh_open_item("item");
        second.refresh_open_item("item");
        let mut told = Vec::new();
        while told.is_empty() {
            if let (to, Event::Notice(_)) = next_event(&events).await {
                told.push(to);
            }
        }
        assert_eq!(told, [second.window()]);

        let mut far = pipes.next().await;
        far.welcome(1).await;
        loop {
            if let EngineLink::Up { again, .. } = next_link(&events).await {
                assert!(again, "the windows open again what they had");
                break;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_connection_that_ends_at_once_keeps_doubling_the_wait() {
        let (dial, mut pipes) = pipes();
        let (_backend, events) = link(
            "halo",
            dial,
            None,
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        let mut waits = Vec::new();
        for connection in 1..=3 {
            let mut far = pipes.next().await;
            far.welcome(connection).await;
            // The engine takes the resync, and fails.
            far.command().await;
            drop(far);
            loop {
                if let EngineLink::Down { retry_in, .. } = next_link(&events).await {
                    waits.push(retry_in.unwrap());
                    break;
                }
            }
        }
        assert_eq!(waits, [QUICK.first, QUICK.first * 2, QUICK.first * 4]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_try_with_no_answer_is_given_up() {
        let (dial, mut pipes) = pipes();
        let backoff = Backoff {
            answer: Duration::from_secs(1),
            ..QUICK
        };
        let (_backend, events) = link(
            "halo",
            dial,
            None,
            backoff,
            &tokio::runtime::Handle::current(),
        );
        // A host that hears the hello and never answers.
        let mut far = pipes.next().await;
        far.hello().await;
        loop {
            if let EngineLink::Down { why, .. } = next_link(&events).await {
                assert_eq!(why, "halo did not answer within 1 s.");
                break;
            }
        }
        // And the next try is made.
        pipes.next().await.welcome(2).await;
        loop {
            if let EngineLink::Up { .. } = next_link(&events).await {
                break;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_reader_can_give_up_a_try_under_way() {
        let (dial, mut pipes) = pipes();
        let backoff = Backoff {
            answer: Duration::from_secs(60),
            ..QUICK
        };
        let (backend, events) = link(
            "halo",
            dial,
            None,
            backoff,
            &tokio::runtime::Handle::current(),
        );
        let mut stuck = pipes.next().await;
        stuck.hello().await;
        backend.reconnect();
        // A new try at once, not after a minute.
        let mut far = pipes.next().await;
        far.welcome(2).await;
        assert_eq!(pipes.dials(), 2);
        loop {
            if let EngineLink::Up { .. } = next_link(&events).await {
                break;
            }
        }
        // Connected, a click on Reconnect leaves the connection be.
        backend.reconnect();
        tokio::time::sleep(QUICK.last).await;
        assert_eq!(pipes.dials(), 2);
        drop((far, stuck));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refused_protocol_stops_the_tries_until_the_reader_asks() {
        let (dial, mut pipes) = pipes();
        let (backend, events) = link(
            "halo",
            dial,
            None,
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        let mut far = pipes.next().await;
        far.hello().await;
        let reason = "Perch 1 speaks protocol 1, and this engine, Perch 2, speaks protocol 2. Update the app.";
        far.answer(Answer::Refused {
            reason: reason.into(),
            about: About::default(),
        })
        .await;
        loop {
            if let EngineLink::Down { why, retry_in, .. } = next_link(&events).await {
                assert_eq!(why, reason);
                assert_eq!(retry_in, None);
                break;
            }
        }
        // Many waits later, still one dial.
        tokio::time::sleep(QUICK.last * 4).await;
        assert_eq!(pipes.dials(), 1);
        backend.reconnect();
        let mut far = pipes.next().await;
        far.welcome(5).await;
        assert_eq!(pipes.dials(), 2);
        loop {
            if let EngineLink::Up { .. } = next_link(&events).await {
                break;
            }
        }
    }

    /// An install that records what it was asked for, and fails with
    /// `failing` while that holds something.
    fn installer(asked: Arc<Mutex<Vec<Platform>>>, failing: Option<Failed>) -> Install {
        Box::new(move |arch, _progress| {
            asked.lock().unwrap().push(arch);
            let failing = failing.clone();
            (
                "0.9.0".to_string(),
                Box::pin(async move { failing.map_or(Ok(()), Err) }),
            )
        })
    }

    #[tokio::test]
    async fn installation_progress_reaches_the_link_before_completion() {
        let (dial, mut pipes) = pipes();
        let finish = Arc::new(tokio::sync::Notify::new());
        let done = finish.clone();
        let expected = crate::backend::InstallProgress {
            phase: "Uploading perchd".into(),
            completed: 25,
            total: Some(100),
        };
        let update = expected.clone();
        let install: Install = Box::new(move |_, progress| {
            let (done, update) = (done.clone(), update.clone());
            (
                "0.9.0".into(),
                Box::pin(async move {
                    progress.send_replace(Some(update));
                    done.notified().await;
                    Ok(())
                }),
            )
        });
        let (_backend, events) = link(
            "halo",
            dial,
            Some(install),
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        *pipes.last.lock().unwrap() = Some(MISSING.into());
        drop(pipes.next().await);
        assert!(matches!(
            next_link(&events).await,
            EngineLink::Connecting { .. }
        ));
        assert!(matches!(
            next_link(&events).await,
            EngineLink::Installing { progress: None, .. }
        ));
        assert!(
            matches!(next_link(&events).await, EngineLink::Installing { progress: Some(p), .. } if p == expected)
        );
        *pipes.last.lock().unwrap() = None;
        finish.notify_one();
        assert!(matches!(
            next_link(&events).await,
            EngineLink::Connecting { .. }
        ));
    }

    const MISSING: &str = "perch-install: Linux aarch64";

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_host_with_no_perchd_has_it_installed_and_is_dialled_again_at_once() {
        let (dial, mut pipes) = pipes();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let backoff = Backoff {
            // Long enough that a wait would show.
            first: Duration::from_secs(30),
            ..QUICK
        };
        let (_backend, events) = link(
            "halo",
            dial,
            Some(installer(asked.clone(), None)),
            backoff,
            &tokio::runtime::Handle::current(),
        );
        *pipes.last.lock().unwrap() = Some(MISSING.into());
        drop(pipes.next().await);
        assert_eq!(
            next_link(&events).await,
            EngineLink::Connecting {
                host: "halo".into()
            }
        );
        assert_eq!(
            next_link(&events).await,
            EngineLink::Installing {
                host: "halo".into(),
                build: "0.9.0".into(),
                progress: None,
            }
        );
        *pipes.last.lock().unwrap() = None;
        assert_eq!(
            next_link(&events).await,
            EngineLink::Connecting {
                host: "halo".into()
            }
        );
        pipes.next().await.welcome(1).await;
        assert!(matches!(next_link(&events).await, EngineLink::Up { .. }));
        assert_eq!(
            *asked.lock().unwrap(),
            [Platform::new(provision::Os::Linux, provision::Arch::Arm64)]
        );
        assert_eq!(pipes.dials(), 2);
    }

    /// An install that went through, with the host still saying it has
    /// none, is not made again and again: the tries stop and say why.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_host_still_missing_perchd_after_an_install_stops_the_tries() {
        let (dial, mut pipes) = pipes();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (backend, events) = link(
            "halo",
            dial,
            Some(installer(asked.clone(), None)),
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        *pipes.last.lock().unwrap() = Some(MISSING.into());
        drop(pipes.next().await);
        drop(pipes.next().await);
        loop {
            if let EngineLink::Down { why, retry_in, .. } = next_link(&events).await {
                assert!(why.contains("still says it has none"), "{why}");
                assert_eq!(retry_in, None);
                break;
            }
        }
        tokio::time::sleep(QUICK.last * 4).await;
        assert_eq!(pipes.dials(), 2);
        assert_eq!(asked.lock().unwrap().len(), 1);
        // Asked to, it tries again, and installs again.
        backend.reconnect();
        drop(pipes.next().await);
        loop {
            if let EngineLink::Installing { .. } = next_link(&events).await {
                break;
            }
        }
        assert_eq!(asked.lock().unwrap().len(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_install_that_fails_waits_as_any_try_does_unless_it_cannot_help() {
        let (dial, mut pipes) = pipes();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (_backend, events) = link(
            "halo",
            dial,
            Some(installer(
                asked.clone(),
                Some(Failed::now("couldn't download it".into())),
            )),
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        *pipes.last.lock().unwrap() = Some(MISSING.into());
        drop(pipes.next().await);
        loop {
            if let EngineLink::Down { why, retry_in, .. } = next_link(&events).await {
                assert_eq!(why, "couldn't download it");
                assert_eq!(retry_in, Some(QUICK.first));
                break;
            }
        }
        // The next try asks again, and installs again.
        drop(pipes.next().await);
        loop {
            if let EngineLink::Down { retry_in, .. } = next_link(&events).await {
                assert_eq!(retry_in, Some(QUICK.first * 2));
                break;
            }
        }
        assert_eq!(asked.lock().unwrap().len(), 2);

        // A host Perch has no perchd for stops the tries.
        let (dial, mut other) = super::tests::pipes();
        let (_backend, events) = link(
            "halo",
            dial,
            Some(installer(asked.clone(), None)),
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        *other.last.lock().unwrap() = Some("perch-install: Linux riscv64".into());
        drop(other.next().await);
        loop {
            if let EngineLink::Down { why, retry_in, .. } = next_link(&events).await {
                assert!(why.contains("[remote] command"), "{why}");
                assert_eq!(retry_in, None);
                break;
            }
        }
        assert_eq!(asked.lock().unwrap().len(), 2);
    }

    /// With a command of the reader's own, nothing is installed: the line
    /// is only why the try failed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn with_a_command_given_nothing_is_installed() {
        let (dial, mut pipes) = pipes();
        // Before the link dials: a dial takes what it will say last as
        // it is made.
        *pipes.last.lock().unwrap() = Some(MISSING.into());
        let (_backend, events) = link(
            "halo",
            dial,
            None,
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        drop(pipes.next().await);
        loop {
            if let EngineLink::Down { why, retry_in, .. } = next_link(&events).await {
                assert_eq!(why, MISSING);
                assert!(retry_in.is_some());
                break;
            }
        }
    }

    #[test]
    fn an_engine_of_another_build_is_replaced_only_idle_and_once() {
        let wanted = ["0123456789ab".to_string()];
        let wanted = Some(&wanted[..]);
        let other = |runs, clients| engine("0.8.0", "ba9876543210", runs, clients);
        assert!(matches!(
            judge(wanted, &other(0, 0), true),
            Judged::Replace(OtherEngine { ref engine, .. }) if engine == "0.8.0 (ba9876543210)"
        ));
        assert!(matches!(
            judge(wanted, &other(0, 0), false),
            Judged::Leave(_)
        ));
        assert!(matches!(
            judge(wanted, &other(2, 0), true),
            Judged::Leave(OtherEngine { runs: 2, .. })
        ));
        assert!(matches!(
            judge(wanted, &other(0, 1), true),
            Judged::Leave(OtherEngine { clients: 1, .. })
        ));
        // Whichever version is the later: a build is the app's or it is
        // not, and one that is not makes way.
        assert!(matches!(
            judge(wanted, &engine("99.0.0", "ba9876543210", 0, 0), true),
            Judged::Replace(_)
        ));
        // The same version built again is another build.
        assert!(matches!(
            judge(wanted, &engine(VERSION, "ba9876543210", 0, 0), true),
            Judged::Replace(_)
        ));
        // One that names no build, or its version, is from before builds
        // were hashes.
        assert!(matches!(
            judge(wanted, &engine(VERSION, "", 0, 0), true),
            Judged::Replace(_)
        ));
        assert!(matches!(
            judge(wanted, &engine(VERSION, VERSION, 0, 0), true),
            Judged::Replace(_)
        ));
        assert_eq!(
            judge(wanted, &engine("0.1.0", "0123456789ab", 3, 3), true),
            Judged::Use
        );
        // An app that carries an engine for each platform takes any of
        // them as its own.
        let carried = ["ba9876543210".to_string(), "0123456789ab".to_string()];
        assert_eq!(judge(Some(&carried[..]), &other(3, 3), true), Judged::Use);
        // A command of the reader's own wants no build: any engine goes.
        assert_eq!(judge(None, &other(0, 0), true), Judged::Use);
    }

    #[test]
    fn a_build_is_said_with_its_version() {
        assert_eq!(engine_said("0.9.0", "0123456789ab"), "0.9.0 (0123456789ab)");
        assert_eq!(engine_said("0.9.0", ""), "0.9.0");
        assert_eq!(engine_said("0.9.0", "0.9.0"), "0.9.0");
        assert_eq!(
            engine_said("0.9.0", "0.9.0-0123456789ab"),
            "0.9.0-0123456789ab"
        );
        assert_eq!(
            wanted_said(&["0123456789ab".to_string()]),
            format!("{VERSION} (0123456789ab)")
        );
        assert_eq!(
            wanted_said(&["0123456789ab".to_string(), "ba9876543210".to_string()]),
            VERSION
        );
    }

    #[test]
    fn the_windows_say_what_a_restart_would_stop() {
        let other = |runs, clients| OtherEngine {
            engine: "0.8.0".into(),
            wanted: "0.9.0".into(),
            runs,
            clients,
        };
        assert_eq!(
            other_said("halo", &other(2, 1)),
            "halo's engine is perchd 0.8.0; restart it to run this app's, perchd 0.9.0 (2 agent runs would be stopped)."
        );
        assert_eq!(
            other_said("halo", &other(1, 0)),
            "halo's engine is perchd 0.8.0; restart it to run this app's, perchd 0.9.0 (1 agent run would be stopped)."
        );
        assert_eq!(
            other_said("halo", &other(0, 1)),
            "halo's engine is perchd 0.8.0; restart it to run this app's, perchd 0.9.0 (1 other client would be disconnected)."
        );
        assert_eq!(
            other_said("halo", &other(0, 0)),
            "halo's engine is perchd 0.8.0; restart it to run this app's, perchd 0.9.0."
        );
    }

    /// The build the app's dials want: its own.
    const WANTED: &str = "0123456789ab";

    /// Another build's, of an older version and of a later one.
    const OTHER: &str = "ba9876543210";

    /// The app's build as the windows say it.
    fn wanted_build() -> String {
        wanted_said(&[WANTED.to_string()])
    }

    fn installed_link() -> (Backend, std::sync::mpsc::Receiver<(WindowId, Event)>, Pipes) {
        let (dial, pipes) = pipes_for(Some(WANTED));
        let (backend, events) = link(
            "halo",
            dial,
            None,
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        (backend, events, pipes)
    }

    /// The next link that is not Connecting.
    async fn next_said(events: &std::sync::mpsc::Receiver<(WindowId, Event)>) -> EngineLink {
        loop {
            let link = next_link(events).await;
            if !matches!(link, EngineLink::Connecting { .. }) {
                return link;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_older_engine_with_nothing_running_is_replaced_at_once() {
        let (_backend, events, mut pipes) = installed_link();
        pipes
            .next()
            .await
            .welcome_as(1, &engine("0.0.1", "0.0.1", 0, 0))
            .await;
        assert_eq!(
            next_said(&events).await,
            EngineLink::Restarting {
                host: "halo".into(),
                build: wanted_build()
            }
        );
        // Kept, so the connection stays up.
        let mut kept = pipes.next().await;
        kept.welcome_as(2, &engine(VERSION, WANTED, 0, 0)).await;
        let EngineLink::Up { other, .. } = next_said(&events).await else {
            panic!("not up");
        };
        assert_eq!(other, None);
        assert_eq!(*pipes.asked.lock().unwrap(), [Replace::No, Replace::IfIdle]);
    }

    /// One replace a connect: an engine still older after it is left
    /// running, rather than replaced again and again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_engine_still_older_after_a_replace_is_left_running() {
        let (_backend, events, mut pipes) = installed_link();
        let older = engine("0.0.1", "0.0.1", 0, 0);
        pipes.next().await.welcome_as(1, &older).await;
        let mut far = pipes.next().await;
        far.welcome_as(2, &older).await;
        loop {
            if let EngineLink::Up { other, .. } = next_said(&events).await {
                assert_eq!(other.map(|other| other.engine), Some("0.0.1".to_string()));
                break;
            }
        }
        assert!(far.command().await.is_some(), "connected");
        assert_eq!(pipes.dials(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_busy_older_engine_is_used_and_restarted_only_when_asked() {
        let (backend, events, mut pipes) = installed_link();
        let mut far = pipes.next().await;
        far.welcome_as(1, &engine("0.0.1", "0.0.1", 2, 1)).await;
        let EngineLink::Up { other, .. } = next_said(&events).await else {
            panic!("not up");
        };
        let other = other.expect("another build");
        assert_eq!((other.runs, other.clients), (2, 1));
        // Connected meanwhile, as to any engine.
        assert!(matches!(
            far.command().await,
            Some((NO_WINDOW, Command::Resync))
        ));
        assert_eq!(pipes.dials(), 1);

        backend.restart_engine();
        assert_eq!(
            next_said(&events).await,
            EngineLink::Restarting {
                host: "halo".into(),
                build: wanted_build()
            }
        );
        // The old connection is let go.
        assert!(far.command().await.is_none());
        // Kept, so the connection stays up.
        let mut kept = pipes.next().await;
        kept.welcome_as(2, &engine(VERSION, WANTED, 0, 0)).await;
        assert!(matches!(
            next_said(&events).await,
            EngineLink::Up { other: None, .. }
        ));
        assert_eq!(*pipes.asked.lock().unwrap(), [Replace::No, Replace::Always]);
        // With nothing offered, a Restart does nothing.
        backend.restart_engine();
        tokio::time::sleep(QUICK.last * 2).await;
        assert_eq!(pipes.dials(), 2);
    }

    /// An engine of a later version than the app's is another build like
    /// any other: with nothing running it makes way for the app's own.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_engine_of_a_later_version_makes_way_too() {
        let (_backend, events, mut pipes) = installed_link();
        let mut far = pipes.next().await;
        far.welcome_as(1, &engine("99.0.0", OTHER, 0, 0)).await;
        assert_eq!(
            next_said(&events).await,
            EngineLink::Restarting {
                host: "halo".into(),
                build: wanted_build()
            }
        );
        let mut kept = pipes.next().await;
        kept.welcome_as(2, &engine(VERSION, WANTED, 0, 0)).await;
        assert!(matches!(
            next_said(&events).await,
            EngineLink::Up { other: None, .. }
        ));
        assert_eq!(*pipes.asked.lock().unwrap(), [Replace::No, Replace::IfIdle]);

        // Busy and refusing the app's protocol, it is left running, with
        // the restart offered.
        let (backend, events, mut pipes) = installed_link();
        let mut far = pipes.next().await;
        far.hello().await;
        far.answer(Answer::Refused {
            reason: "Update the app.".into(),
            about: About {
                protocol: PROTOCOL + 1,
                ..engine("99.0.0", OTHER, 1, 0)
            },
        })
        .await;
        let EngineLink::Down {
            other, retry_in, ..
        } = next_said(&events).await
        else {
            panic!("not down");
        };
        assert_eq!(other.unwrap().engine, format!("99.0.0 ({OTHER})"));
        assert_eq!(retry_in, None);
        backend.restart_engine();
        assert_eq!(
            next_said(&events).await,
            EngineLink::Restarting {
                host: "halo".into(),
                build: wanted_build()
            }
        );
        pipes.next().await;
        assert_eq!(*pipes.asked.lock().unwrap(), [Replace::No, Replace::Always]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_older_engine_that_refuses_the_app_is_offered_a_restart() {
        let (backend, events, mut pipes) = installed_link();
        let mut far = pipes.next().await;
        far.hello().await;
        far.answer(Answer::Refused {
            reason: "Update the engine.".into(),
            about: About {
                protocol: PROTOCOL - 1,
                ..engine("0.0.1", "0.0.1", 1, 0)
            },
        })
        .await;
        let EngineLink::Down {
            other, retry_in, ..
        } = next_said(&events).await
        else {
            panic!("not down");
        };
        assert_eq!(other.unwrap().runs, 1);
        assert_eq!(retry_in, None);
        backend.restart_engine();
        assert!(matches!(
            next_said(&events).await,
            EngineLink::Restarting { .. }
        ));
        // Kept, so the connection stays up.
        let mut kept = pipes.next().await;
        kept.welcome_as(2, &engine(VERSION, WANTED, 0, 0)).await;
        assert!(matches!(
            next_said(&events).await,
            EngineLink::Up { other: None, .. }
        ));
        assert_eq!(*pipes.asked.lock().unwrap(), [Replace::No, Replace::Always]);
    }

    /// An engine that let the app in, and came into use between the welcome
    /// and the replace: the bridge says it is busy, and the app connects to
    /// it at once, with the restart offered, as to one busy from the first.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_replace_that_finds_the_engine_busy_connects_to_it() {
        let (_backend, events, mut pipes) = installed_link();
        let mut first = pipes.next().await;
        // What the next dial, the replace, says as it ends: busy by now.
        *pipes.last.lock().unwrap() = Some("perch-busy: 0.0.1 3 0".into());
        first.welcome_as(1, &engine("0.0.1", "0.0.1", 0, 0)).await;
        assert!(matches!(
            next_said(&events).await,
            EngineLink::Restarting { .. }
        ));
        let replace = pipes.next().await;
        *pipes.last.lock().unwrap() = None;
        drop(replace);
        let mut far = pipes.next().await;
        far.welcome_as(3, &engine("0.0.1", "0.0.1", 3, 0)).await;
        let EngineLink::Up { other, .. } = next_said(&events).await else {
            panic!("not up");
        };
        let other = other.expect("another build");
        assert_eq!((other.engine.as_str(), other.runs), ("0.0.1", 3));
        assert!(far.command().await.is_some(), "connected");
        assert_eq!(
            *pipes.asked.lock().unwrap(),
            [Replace::No, Replace::IfIdle, Replace::No]
        );
    }

    /// An engine that refused the app's protocol, and came into use before
    /// the replace: not connected, with the restart offered.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_replace_that_finds_a_refusing_engine_busy_offers_a_restart() {
        let (backend, events, mut pipes) = installed_link();
        let mut first = pipes.next().await;
        // What the next dial, the replace, says as it ends.
        *pipes.last.lock().unwrap() = Some("perch-busy: 0.0.1 3 0".into());
        first.hello().await;
        first
            .answer(Answer::Refused {
                reason: "Update the engine.".into(),
                about: About {
                    protocol: PROTOCOL - 1,
                    ..engine("0.0.1", "0.0.1", 0, 0)
                },
            })
            .await;
        assert!(matches!(
            next_said(&events).await,
            EngineLink::Restarting { .. }
        ));
        drop(pipes.next().await);
        let EngineLink::Down {
            other, retry_in, ..
        } = next_said(&events).await
        else {
            panic!("not down");
        };
        let other = other.unwrap();
        assert_eq!((other.engine.as_str(), other.runs), ("0.0.1", 3));
        assert_eq!(retry_in, None);
        tokio::time::sleep(QUICK.last * 2).await;
        assert_eq!(pipes.dials(), 2);
        *pipes.last.lock().unwrap() = None;
        backend.restart_engine();
        drop(pipes.next().await);
        assert_eq!(
            *pipes.asked.lock().unwrap(),
            [Replace::No, Replace::IfIdle, Replace::Always]
        );
    }

    /// A replace, or a restart the reader asked for, whose dial finds the
    /// build wanted gone from the host installs it and dials again as it
    /// was asked, still replacing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_install_on_the_way_to_a_replace_keeps_the_replace() {
        for restart in [false, true] {
            let (dial, mut pipes) = pipes_for(Some(WANTED));
            let installs = Arc::new(Mutex::new(Vec::new()));
            let (backend, events) = link(
                "halo",
                dial,
                Some(installer(installs.clone(), None)),
                QUICK,
                &tokio::runtime::Handle::current(),
            );
            let runs = usize::from(restart);
            let mut first = pipes.next().await;
            // What the next dial, the replace, says as it ends.
            if !restart {
                *pipes.last.lock().unwrap() = Some(MISSING.into());
            }
            first
                .welcome_as(1, &engine("0.0.1", "0.0.1", runs, 0))
                .await;
            if restart {
                assert!(matches!(next_said(&events).await, EngineLink::Up { .. }));
                *pipes.last.lock().unwrap() = Some(MISSING.into());
                backend.restart_engine();
            }
            assert!(matches!(
                next_said(&events).await,
                EngineLink::Restarting { .. }
            ));
            let missing = pipes.next().await;
            *pipes.last.lock().unwrap() = None;
            drop(missing);
            assert!(matches!(
                next_said(&events).await,
                EngineLink::Installing { .. }
            ));
            let mut kept = pipes.next().await;
            kept.welcome_as(2, &engine(VERSION, WANTED, 0, 0)).await;
            assert!(matches!(
                next_said(&events).await,
                EngineLink::Up { other: None, .. }
            ));
            let asked = if restart {
                Replace::Always
            } else {
                Replace::IfIdle
            };
            assert_eq!(*pipes.asked.lock().unwrap(), [Replace::No, asked, asked]);
            assert_eq!(installs.lock().unwrap().len(), 1);
            drop(first);
        }
    }

    /// A replace, or a restart the reader asked for, whose dial fails
    /// before the host's bridge runs, as when the host's own `perchd` is
    /// being rebuilt under it, is still a replace on the try after.
    /// ssh starts whether or not it reaches the host, so a host that
    /// can't be reached is a stream that ends with nothing said. A
    /// replace, or the reader's restart, that ends so is still asked of
    /// the next try: the engine it was for is still there.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_try_that_ends_before_the_engine_answers_keeps_the_replace() {
        for restart in [false, true] {
            let (backend, events, mut pipes) = installed_link();
            let runs = usize::from(restart);
            let mut first = pipes.next().await;
            first.welcome_as(1, &engine("0.0.1", OTHER, runs, 0)).await;
            if restart {
                assert!(matches!(next_said(&events).await, EngineLink::Up { .. }));
                backend.restart_engine();
            }
            assert!(matches!(
                next_said(&events).await,
                EngineLink::Restarting { .. }
            ));
            // The try that was to replace it never reaches the host: its
            // stream ends with nothing said.
            drop(pipes.next().await);
            assert!(matches!(
                next_said(&events).await,
                EngineLink::Down {
                    retry_in: Some(_),
                    ..
                }
            ));
            let mut kept = pipes.next().await;
            kept.welcome_as(2, &engine(VERSION, WANTED, 0, 0)).await;
            assert!(matches!(
                next_said(&events).await,
                EngineLink::Up { other: None, .. }
            ));
            let asked = if restart {
                Replace::Always
            } else {
                Replace::IfIdle
            };
            assert_eq!(*pipes.asked.lock().unwrap(), [Replace::No, asked, asked]);
            drop(first);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dial_that_fails_on_the_way_to_a_replace_keeps_the_replace() {
        for restart in [false, true] {
            let (backend, events, mut pipes) = installed_link();
            let runs = usize::from(restart);
            let mut first = pipes.next().await;
            if !restart {
                *pipes.failing.lock().unwrap() = Some("no perchd there just now".into());
            }
            first
                .welcome_as(1, &engine("0.0.1", "0.0.1", runs, 0))
                .await;
            if restart {
                assert!(matches!(next_said(&events).await, EngineLink::Up { .. }));
                *pipes.failing.lock().unwrap() = Some("no perchd there just now".into());
                backend.restart_engine();
            }
            assert!(matches!(
                next_said(&events).await,
                EngineLink::Restarting { .. }
            ));
            let EngineLink::Down { why, .. } = next_said(&events).await else {
                panic!("not down");
            };
            assert_eq!(why, "no perchd there just now");
            *pipes.failing.lock().unwrap() = None;
            let mut kept = pipes.next().await;
            kept.welcome_as(2, &engine(VERSION, WANTED, 0, 0)).await;
            assert!(matches!(
                next_said(&events).await,
                EngineLink::Up { other: None, .. }
            ));
            let asked = if restart {
                Replace::Always
            } else {
                Replace::IfIdle
            };
            let dials = pipes.asked.lock().unwrap().clone();
            assert_eq!(dials[0], Replace::No);
            assert!(dials[1..].iter().all(|dial| *dial == asked), "{dials:?}");
            assert!(dials.len() >= 3, "{dials:?}");
            drop(first);
        }
    }

    /// A try now asked for while a dial is still on its way gives that
    /// dial up for another, rather than waiting to end the connection it
    /// makes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_try_now_during_a_slow_dial_does_not_end_the_connection_after_it() {
        let (dial, mut pipes) = pipes_for(Some(WANTED));
        *pipes.failing.lock().unwrap() = Some("unreachable".into());
        let backoff = Backoff {
            // Long enough that only the reader's ask ends the wait.
            first: Duration::from_secs(30),
            ..QUICK
        };
        let (backend, events) = link(
            "halo",
            dial,
            None,
            backoff,
            &tokio::runtime::Handle::current(),
        );
        assert!(matches!(next_said(&events).await, EngineLink::Down { .. }));
        // The second dial stalls, and the reader asks again during it.
        *pipes.failing.lock().unwrap() = None;
        *pipes.stall.lock().unwrap() = Duration::from_millis(300);
        backend.reconnect();
        for _ in 0..200 {
            if pipes.dials() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(pipes.dials(), 2);
        *pipes.stall.lock().unwrap() = Duration::ZERO;
        backend.reconnect();
        // The dial given up is dropped before it has a stream; the one
        // after it connects, and stays connected.
        let mut kept = pipes.next().await;
        kept.welcome_as(1, &engine(VERSION, WANTED, 0, 0)).await;
        assert!(matches!(
            next_said(&events).await,
            EngineLink::Up { other: None, .. }
        ));
        assert_eq!(pipes.dials(), 3);
        tokio::time::sleep(Duration::from_millis(200)).await;
        while let Ok(event) = events.try_recv() {
            assert!(
                !matches!(event, (_, Event::Link(EngineLink::Down { .. }))),
                "the connection was ended"
            );
        }
        drop(kept);
    }

    /// An engine from before the frozen handshake says nothing of itself,
    /// and could not hear a stop: the reader is told to stop it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_engine_from_before_the_frozen_handshake_is_not_replaced() {
        let (_backend, events, mut pipes) = installed_link();
        let mut far = pipes.next().await;
        far.hello().await;
        far.answer(Answer::Refused {
            reason: "Update the engine.".into(),
            about: About::default(),
        })
        .await;
        let EngineLink::Down { why, other, .. } = next_said(&events).await else {
            panic!("not down");
        };
        assert!(why.contains("stop perchd serve on halo"), "{why}");
        assert_eq!(other, None);
        tokio::time::sleep(QUICK.last * 2).await;
        assert_eq!(pipes.dials(), 1);
    }

    /// With a command of the reader's own, any engine is used as it is.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn with_a_command_given_no_engine_is_replaced() {
        let (dial, mut pipes) = pipes();
        let (_backend, events) = link(
            "halo",
            dial,
            None,
            QUICK,
            &tokio::runtime::Handle::current(),
        );
        let mut kept = pipes.next().await;
        kept.welcome_as(1, &engine("0.0.1", "0.0.1", 0, 0)).await;
        assert!(matches!(
            next_said(&events).await,
            EngineLink::Up { other: None, .. }
        ));
        assert_eq!(*pipes.asked.lock().unwrap(), [Replace::No]);
    }

    /// The connection script, run as ssh would have the login shell run
    /// it, by each shell there is here: bash and zsh on macOS, fish when
    /// installed. It runs the bridge of the build named for what the host
    /// is, under the Perch home a login shell sees, and says what the host
    /// is when it has none.
    #[test]
    fn the_connection_script_runs_in_every_login_shell() {
        use std::os::unix::fs::PermissionsExt;
        let shells = provision::tests::shells();
        assert!(!shells.is_empty());
        for shell in shells {
            let home = tempfile::tempdir().unwrap();
            let bin = home.path().join(".perch/bin");
            std::fs::create_dir_all(&bin).unwrap();
            let fake = bin.join("perchd-0123456789ab");
            std::fs::write(&fake, "#!/bin/sh\necho \"ran $0 $*\"\n").unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
            let run_as = |builds: &[(&str, &str)], replace: Replace| {
                let builds: Vec<(String, String)> = builds
                    .iter()
                    .map(|(platform, build)| (platform.to_string(), build.to_string()))
                    .collect();
                std::process::Command::new(&shell)
                    .arg("-c")
                    .arg(provision::connect_command(&builds, replace))
                    .env_clear()
                    .env("HOME", home.path())
                    .env("SHELL", &shell)
                    .env("PATH", "/usr/bin:/bin")
                    .output()
                    .unwrap()
            };
            let run = |builds: &[(&str, &str)]| run_as(builds, Replace::No);
            let ran = run(&[(provision::ANY, "0123456789ab")]);
            assert_eq!(
                String::from_utf8_lossy(&ran.stdout).trim(),
                format!("ran {} bridge", fake.display()),
                "{shell}: {}",
                String::from_utf8_lossy(&ran.stderr)
            );
            assert!(ran.status.success(), "{shell}");
            let ran = run_as(&[(provision::ANY, "0123456789ab")], Replace::IfIdle);
            assert_eq!(
                String::from_utf8_lossy(&ran.stdout).trim(),
                format!("ran {} bridge --replace-if-idle", fake.display()),
                "{shell}: {}",
                String::from_utf8_lossy(&ran.stderr)
            );

            // Of an app's engines, the host runs the one for what it is.
            let Some(here) = Platform::here() else {
                continue;
            };
            let carried: Vec<(String, &str)> = Platform::ALL
                .into_iter()
                .map(|platform| {
                    let build = if platform == here {
                        "0123456789ab"
                    } else {
                        "ba9876543210"
                    };
                    (platform.key(), build)
                })
                .collect();
            let carried: Vec<(&str, &str)> = carried
                .iter()
                .map(|(platform, build)| (platform.as_str(), *build))
                .collect();
            let ran = run_as(&carried, Replace::Always);
            assert_eq!(
                String::from_utf8_lossy(&ran.stdout).trim(),
                format!("ran {} bridge --replace", fake.display()),
                "{shell}: {}",
                String::from_utf8_lossy(&ran.stderr)
            );
            // With none for it, or none of that build there, it says so.
            let others: Vec<(&str, &str)> = carried
                .iter()
                .copied()
                .filter(|(platform, _)| *platform != here.key())
                .collect();
            assert_eq!(
                run(&others).status.code(),
                Some(provision::INSTALL_EXIT),
                "{shell}"
            );
            assert_eq!(
                run(&[]).status.code(),
                Some(provision::INSTALL_EXIT),
                "{shell}"
            );

            let missing = run(&[(provision::ANY, "ba9876543210")]);
            assert_eq!(
                missing.status.code(),
                Some(provision::INSTALL_EXIT),
                "{shell}"
            );
            let said = String::from_utf8_lossy(&missing.stderr);
            let last = said.lines().last().unwrap_or_default();
            let uname = |flag: &str| {
                let out = std::process::Command::new("uname")
                    .arg(flag)
                    .output()
                    .unwrap();
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            };
            assert_eq!(
                last,
                format!("perch-install: {} {}", uname("-s"), uname("-m")),
                "{shell}"
            );
            assert!(provision::install_line(last).is_some());
        }
    }
}
