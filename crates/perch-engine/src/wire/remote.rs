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
//!   sit empty.
//! - Tries are made 1 s apart at first, doubling to 30 s between, and the
//!   reader can ask for one now. A refusal of the app's protocol stops
//!   them: trying again cannot help until one side is updated.
//! - A connection made asks for everything again (`Command::Resync`), after
//!   telling the engine of every window open, so the windows catch up on
//!   what happened meanwhile. Each window reopens its item when it hears
//!   it is connected again (`EngineLink::Up`).
//!
//! How the connection stands goes to the windows as `Event::Link`, with
//! the engine's own events, in order.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::UnboundedSender;

use super::{handshake, WireError};
use crate::backend::client::Navigations;
use crate::backend::{Backend, Command, EngineLink, Event, WindowId, NO_WINDOW};

/// How often a refused command is said, at most: a pointer moving over
/// code asks for hovers many times a second.
const REFUSALS_APART: Duration = Duration::from_secs(2);

/// What `[remote] command` is when the file says nothing.
pub const DEFAULT_COMMAND: &str = "perchd bridge";

/// A byte stream to an engine.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Stream for T {}

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A stream to an engine, as a dial made it.
pub struct Dialed {
    pub stream: Box<dyn Stream>,
    /// Let go of what carries the stream, once the stream is done with,
    /// and say what it said last on its way out, such as ssh's last line
    /// of stderr: why it ended, most often.
    pub hang_up: Box<dyn FnOnce() -> BoxFuture<Option<String>> + Send>,
}

/// How a stream to the engine is made: `ssh` in the app, a pipe in memory
/// in a test. Err says why none could be.
pub type Dial = Box<dyn FnMut() -> BoxFuture<Result<Dialed, String>> + Send>;

/// The waits between tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub first: Duration,
    pub last: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            first: Duration::from_secs(1),
            last: Duration::from_secs(30),
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
}

struct Linked {
    /// Where commands go while connected.
    to: Option<UnboundedSender<(WindowId, Command)>>,
    /// The client's windows that are open, to open again in the engine
    /// each time it is connected.
    windows: BTreeSet<WindowId>,
    /// When a refusal was last said.
    refused_at: Option<Instant>,
    /// What the windows sent before the first try ended, while it is
    /// under way; `None` after.
    starting: Option<Vec<(WindowId, Command)>>,
}

impl Default for Linked {
    fn default() -> Self {
        Linked {
            to: None,
            windows: BTreeSet::new(),
            refused_at: None,
            starting: Some(Vec::new()),
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
    /// connection. A window opening or closing is kept either way.
    pub(crate) fn send(&self, window: WindowId, command: Command) -> Result<(), ()> {
        let mut linked = self.linked();
        match command {
            Command::WindowOpened if window != NO_WINDOW => {
                linked.windows.insert(window);
            }
            Command::WindowClosed => {
                linked.windows.remove(&window);
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
            Command::WindowOpened | Command::WindowClosed | Command::Resync
        ) {
            return Ok(());
        }
        if let Some(starting) = linked.starting.as_mut() {
            starting.push((window, command));
            return Ok(());
        }
        let now = Instant::now();
        if linked
            .refused_at
            .is_none_or(|at| now.duration_since(at) >= REFUSALS_APART)
        {
            linked.refused_at = Some(now);
            let _ = self
                .events
                .send((window, Event::Notice(refused(&self.host))));
        }
        Err(())
    }

    /// Try now, unless connected.
    pub(crate) fn retry_now(&self) {
        if self.linked().to.is_none() {
            self.retry.notify_one();
        }
    }

    /// Connected: the windows are opened in the engine, then everything
    /// is asked for again, before any command from a window can go.
    fn up(
        &self,
        welcome: &super::Welcome,
        to: UnboundedSender<(WindowId, Command)>,
        again: bool,
    ) -> bool {
        let mut linked = self.linked();
        self.navigations.reconnected(welcome.connection);
        for &window in &linked.windows {
            let _ = to.send((window, Command::WindowOpened));
        }
        let _ = to.send((NO_WINDOW, Command::Resync));
        for sent in linked.starting.take().into_iter().flatten() {
            let _ = to.send(sent);
        }
        linked.to = Some(to);
        linked.refused_at = None;
        self.say(EngineLink::Up {
            host: self.host.clone(),
            welcome: welcome.clone(),
            again,
        })
    }

    fn down(&self) {
        let mut linked = self.linked();
        linked.to = None;
        linked.starting = None;
    }

    /// Tell the windows how the connection stands. False once nobody is
    /// listening: the app has gone.
    fn say(&self, link: EngineLink) -> bool {
        self.events.send((NO_WINDOW, Event::Link(link))).is_ok()
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
    backoff: Backoff,
    runtime: &tokio::runtime::Handle,
) -> (Backend, std::sync::mpsc::Receiver<(WindowId, Event)>) {
    let (events, received) = std::sync::mpsc::channel();
    let link = Arc::new(Link {
        host: host.to_string(),
        navigations: Arc::default(),
        events,
        state: Mutex::default(),
        retry: tokio::sync::Notify::new(),
    });
    runtime.spawn(keep_connected(link.clone(), dial, backoff));
    (Backend::linked(link), received)
}

/// Make the connection, and make it again each time it is lost, until the
/// app stops listening.
async fn keep_connected(link: Arc<Link>, mut dial: Dial, backoff: Backoff) {
    let host = link.host.clone();
    if !link.say(EngineLink::Connecting { host: host.clone() }) {
        return;
    }
    let mut wait = backoff.first;
    let mut ever = false;
    loop {
        let why = match dial().await {
            Err(why) => why,
            Ok(Dialed { stream, hang_up }) => match handshake(stream).await {
                Ok((welcome, session)) => {
                    tracing::info!(
                        host,
                        engine = welcome.engine_version,
                        engine_host = welcome.host,
                        "connected to the engine"
                    );
                    let (to, outgoing) = tokio::sync::mpsc::unbounded_channel();
                    let ended = session.carry(outgoing, link.navigations(), link.events.clone());
                    if !link.up(&welcome, to, ever) {
                        return;
                    }
                    ever = true;
                    wait = backoff.first;
                    let _ = ended.await;
                    link.down();
                    hang_up()
                        .await
                        .unwrap_or_else(|| "The connection was lost.".to_string())
                }
                Err(WireError::Refused(reason)) => {
                    let _ = hang_up().await;
                    tracing::warn!(host, %reason, "the engine refused the app");
                    link.down();
                    if !link.say(EngineLink::Down {
                        host: host.clone(),
                        why: reason,
                        retry_in: None,
                        ever,
                    }) {
                        return;
                    }
                    link.retry.notified().await;
                    wait = backoff.first;
                    continue;
                }
                Err(error) => hang_up().await.unwrap_or_else(|| error.to_string()),
            },
        };
        tracing::warn!(host, %why, ?wait, "not connected to the engine");
        link.down();
        if !link.say(EngineLink::Down {
            host: host.clone(),
            why,
            retry_in: Some(wait),
            ever,
        }) {
            return;
        }
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = link.retry.notified() => {}
        }
        wait = backoff.after(wait);
    }
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
        None
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

/// ssh's arguments to run `command` on `host`: no terminal, so nothing
/// but the wire is on its stdout, and a check every 15 s that the host is
/// still there, given up after three go unanswered.
pub fn ssh_args(host: &str, command: &str) -> Vec<String> {
    [
        "-T",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        host,
    ]
    .into_iter()
    .map(str::to_string)
    .chain([in_login_shell(command)])
    .collect()
}

/// Dial `host` with ssh, which runs `command` there. ssh's stderr goes to
/// the log, and its last line is why a connection ended.
pub fn ssh(host: &str, command: &str) -> Dial {
    let (host, command) = (host.to_string(), command.to_string());
    Box::new(move || {
        let (host, command) = (host.clone(), command.clone());
        Box::pin(async move {
            if let Some(why) = bad_host(&host) {
                return Err(why);
            }
            let mut child = tokio::process::Command::new("ssh")
                .args(ssh_args(&host, &command))
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
                let (last, host) = (last.clone(), host.clone());
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
                            *last = Some(line.to_string());
                        }
                    }
                }
            });
            Ok(Dialed {
                stream: Box::new(tokio::io::join(stdout, stdin)),
                hang_up: Box::new(move || {
                    Box::pin(async move {
                        // Most often ssh has gone already, or is going,
                        // and its last words are on their way.
                        if tokio::time::timeout(Duration::from_secs(2), child.wait())
                            .await
                            .is_err()
                        {
                            let _ = child.kill().await;
                        }
                        let _ = tokio::time::timeout(Duration::from_secs(1), said).await;
                        last.lock().ok().and_then(|last| last.clone())
                    })
                }),
            })
        })
    })
}

/// The app's connection to the engine on `host`, by ssh running `command`
/// there, carried by a runtime of the client's own. With no runtime to be
/// had, a handle with nothing behind it, and a status that says why.
pub fn over_ssh(
    host: &str,
    command: &str,
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
    link(
        host,
        ssh(host, command),
        Backoff::default(),
        runtime.handle(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{encode, next_message, Answer, CommandLine, Hello, Welcome, PROTOCOL};
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
    fn ssh_is_given_the_host_and_a_command_for_the_login_shell() {
        assert_eq!(
            ssh_args("halo", DEFAULT_COMMAND),
            [
                "-T",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
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
            self.hello().await;
            self.answer(Answer::Welcome(Welcome {
                protocol: PROTOCOL,
                engine_version: "0".into(),
                connection,
                host: "halo".into(),
                home: "/home/chris".into(),
                data_root: "/home/chris/.perch".into(),
                clients: 0,
                runs: 0,
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

    /// A dial that hands the client one end of a pipe in memory and the
    /// test the other, or fails with `why` while `refusing` holds it.
    struct Pipes {
        far: tokio::sync::mpsc::UnboundedReceiver<Far>,
        dials: Arc<std::sync::atomic::AtomicUsize>,
        failing: Arc<Mutex<Option<String>>>,
    }

    fn pipes() -> (Dial, Pipes) {
        let (to_test, far) = tokio::sync::mpsc::unbounded_channel();
        let dials = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failing = Arc::new(Mutex::new(None::<String>));
        let dial: Dial = Box::new({
            let (dials, failing) = (dials.clone(), failing.clone());
            move || {
                dials.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let failing = failing.lock().unwrap().clone();
                let to_test = to_test.clone();
                Box::pin(async move {
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
                        hang_up: Box::new(|| Box::pin(async { None })),
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
        let (backend, events) = link("halo", dial, QUICK, &tokio::runtime::Handle::current());
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
            Some((NO_WINDOW, Command::Resync))
        ));
        assert!(far.quiet(Duration::from_millis(100)).await);
        loop {
            match next_event(&events).await {
                (_, Event::Link(EngineLink::Up { again, welcome, .. })) => {
                    assert!(!again);
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_lost_connection_is_made_again_and_the_windows_resync() {
        let (dial, mut pipes) = pipes();
        let (backend, events) = link("halo", dial, QUICK, &tokio::runtime::Handle::current());
        let (first, second) = (backend.open_window(), backend.open_window());
        // Asked for as the app starts, before the first try is through:
        // it goes once the windows are open and everything asked for.
        first.refresh_open_item("item");
        let mut far = pipes.next().await;
        far.welcome(1).await;
        let mut opened = Vec::new();
        for _ in 0..4 {
            opened.push(far.command().await.unwrap());
        }
        assert!(matches!(opened[2], (NO_WINDOW, Command::Resync)));
        assert!(matches!(opened[3], (w, Command::RefreshOpenItem { .. }) if w == first.window()));
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refused_protocol_stops_the_tries_until_the_reader_asks() {
        let (dial, mut pipes) = pipes();
        let (backend, events) = link("halo", dial, QUICK, &tokio::runtime::Handle::current());
        let mut far = pipes.next().await;
        far.hello().await;
        let reason = "Perch 1 speaks protocol 1, and this engine, Perch 2, speaks protocol 2. Update the app.";
        far.answer(Answer::Refused {
            reason: reason.into(),
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
}
