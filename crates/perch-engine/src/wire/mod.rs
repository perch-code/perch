//! The engine over a byte stream. One JSON message a line, on any duplex
//! stream: a socket, ssh's stdio, or a pipe in memory.
//!
//! The client sends a `Hello`; the engine answers `Welcome`, or `Refused`
//! when the protocols differ, before any command runs. Then the client
//! sends `{ window, command }` lines and the engine `{ window, event }`
//! lines, the window ids the client's own (see `backend::connections`).
//! A line that does not parse is dropped, with the reason in the log, and
//! the lines after it still go: framing is by the line. An empty line is
//! a keepalive, which the client sends every `KEEPALIVE` and the engine
//! reads past; a client the engine has heard nothing from in `IDLE` is
//! hung up on. Otherwise only a handshake that fails, or the stream
//! itself, ends a connection. Logs never go on the stream.
//!
//! The types and their wire form live in this crate together so they
//! cannot drift apart: what goes over is `Command` and `Event` as serde
//! writes them.

pub mod bytes;
pub mod pairs;
pub mod provision;
pub mod remote;

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf,
};
use tokio::time::Instant;

use crate::backend::{Backend, CheckConfig, Command, Engine, Event, Lifetime, WindowId};

/// The wire's version. A change to `Command` or `Event` that an older
/// peer would read wrong, or not at all, moves it on. 2: `Welcome` counts
/// the engine's other clients and its runs. 3: scope picks carry the
/// handle's ids, a resync's runs and kept settings replace a window's.
/// 4: keepalive lines, and a resync's waiting approvals. 5: the hello and
/// its answer are frozen (see `Hello`), a hello can ask a host's engine to
/// stop, and the answers name the engine's build. 6: a host's engine
/// sends its host checks. 7: Add repository from what was typed, a path
/// or a repository to clone.
pub const PROTOCOL: u32 = 7;

/// The version of Perch this is, as each side names itself.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The longest line read. A longer one is read to its end and dropped, so
/// a peer that never sends a newline cannot fill memory.
pub const MAX_LINE: usize = 256 << 20;

/// How much a client may fall behind in what it is sent before the engine
/// hangs up on it. A laptop asleep on a half-open connection would
/// otherwise have a night of events kept for it; a client that comes back
/// reconnects and resyncs.
pub const MAX_BACKLOG: usize = 256 << 20;

/// How long the engine waits, once a client has gone, for the last of
/// what it was sent to be written.
const LAST_WRITES: Duration = Duration::from_secs(5);

/// How often a client says it is still there, with an empty line, when it
/// has nothing else to say.
pub const KEEPALIVE: Duration = Duration::from_secs(15);

/// How long the engine waits to hear anything from a client before it
/// hangs up. A laptop asleep, or a network gone, leaves a connection
/// half-open, which TCP gives up on only after hours; until then the
/// client's windows, and the language servers they hold, would stay.
pub const IDLE: Duration = Duration::from_secs(60);

/// How far a client may fall behind, how long its last writes get, and
/// how long it may say nothing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) backlog: usize,
    pub(crate) last_writes: Duration,
    pub(crate) idle: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            backlog: MAX_BACKLOG,
            last_writes: LAST_WRITES,
            idle: IDLE,
        }
    }
}

/// The client's first line.
///
/// `Hello` and `Answer` are frozen from protocol 5: a later protocol only
/// adds fields that may be left out, and never renames or removes one, so
/// an engine of any protocol can read what a client of any other asks of
/// it and say who it is. That is how an app updated past its host's engine
/// tells the engine is older, and asks it to stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub app_version: String,
    /// What the client wants. Left out by a client before protocol 5,
    /// which only ever connected.
    #[serde(default)]
    pub intent: Intent,
}

impl Hello {
    /// This client's hello, asking to connect.
    pub fn connect() -> Hello {
        Hello {
            protocol: PROTOCOL,
            app_version: VERSION.to_string(),
            intent: Intent::Connect,
        }
    }
}

/// What a client says hello for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Intent {
    /// To be let in.
    #[default]
    Connect,
    /// For the engine to stop as SIGTERM stops it, whatever protocol the
    /// client speaks: `perchd bridge --replace`, making way for its own
    /// build. Only an engine under `perchd serve` stops, and with
    /// `if_idle`, only one with no runs and no clients. One of
    /// `unless_build` stays, as it is what the client would start, and so
    /// does one of a newer version than the hello's `app_version`: the
    /// client judged an engine older some time before it asked, and the
    /// engine may have been replaced by a newer one since.
    Stop {
        if_idle: bool,
        #[serde(default)]
        unless_build: Option<String>,
    },
}

/// Who an engine is and what it has, in every answer to a hello but a
/// welcome, which says it its own way. Empty from an engine before
/// protocol 5.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct About {
    pub engine_version: String,
    pub protocol: u32,
    /// The `<build>` of the `perchd-<build>` it runs as, or its version
    /// when it was not installed by the app.
    pub build: String,
    /// Agent runs queued or under way.
    pub runs: usize,
    /// Clients connected, not counting the one asking.
    pub clients: usize,
}

/// The engine letting a client in, and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub protocol: u32,
    pub engine_version: String,
    /// The engine's number for this connection, never another's: the
    /// client's attempt ids carry it.
    pub connection: u64,
    /// The engine's machine, by its host name.
    pub host: String,
    /// The engine's home directory: what `~` means in its paths.
    pub home: PathBuf,
    /// Where the engine keeps its checkouts and runs.
    pub data_root: PathBuf,
    /// How many other clients were connected as this one was let in.
    pub clients: usize,
    /// How many agent runs were queued or under way then.
    pub runs: usize,
    /// The engine's build, as `About::build`. Empty from one before
    /// protocol 5.
    #[serde(default)]
    pub build: String,
}

impl Welcome {
    /// What the welcome says of the engine, as a refusal says it.
    pub fn about(&self) -> About {
        About {
            engine_version: self.engine_version.clone(),
            protocol: self.protocol,
            build: self.build.clone(),
            runs: self.runs,
            clients: self.clients,
        }
    }
}

/// The engine's answer to a `Hello`. Frozen, as `Hello` is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    Welcome(Welcome),
    Refused {
        reason: String,
        #[serde(flatten)]
        about: About,
    },
    /// Asked to stop if idle, and not: it has runs or other clients.
    Busy(About),
    /// Stopping, as asked: its socket goes first and its home once its
    /// runs are stopped and recorded.
    Stopping(About),
    /// Asked to stop unless of a build, and it is that build.
    Staying(About),
}

/// A command line, from the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandLine {
    pub window: WindowId,
    pub command: Command,
}

/// An event line, from the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLine {
    pub window: WindowId,
    pub event: Event,
}

/// Why a connection ended or never began.
#[derive(Debug)]
pub enum WireError {
    Io(std::io::Error),
    /// A line that is not the message it should be.
    Malformed(String),
    /// The other side hung up before the handshake was done.
    Closed,
    /// The engine would not let the client in, why, and what it said of
    /// itself.
    Refused(String, About),
    /// The client fell more than `MAX_BACKLOG` behind.
    Behind,
    /// Nothing came from the client, not even a keepalive, in `IDLE`.
    Silent,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Io(error) => write!(f, "{error}"),
            WireError::Malformed(why) => write!(f, "a malformed line: {why}"),
            WireError::Closed => f.write_str("the connection closed during the handshake"),
            WireError::Refused(reason, _) => f.write_str(reason),
            WireError::Behind => f.write_str("the client fell too far behind"),
            WireError::Silent => f.write_str("the client said nothing for too long"),
        }
    }
}

impl std::error::Error for WireError {}

impl From<std::io::Error> for WireError {
    fn from(error: std::io::Error) -> Self {
        WireError::Io(error)
    }
}

/// One message as a line, with its newline.
fn encode(message: &impl Serialize) -> Result<Vec<u8>, WireError> {
    let mut line =
        serde_json::to_vec(message).map_err(|error| WireError::Malformed(error.to_string()))?;
    line.push(b'\n');
    Ok(line)
}

/// Write one message and its newline.
async fn write_line<W: AsyncWrite + Unpin>(
    write: &mut W,
    message: &impl Serialize,
) -> Result<(), WireError> {
    let line = encode(message)?;
    write.write_all(&line).await?;
    write.flush().await?;
    Ok(())
}

/// What `next_line` read.
#[derive(Debug, PartialEq, Eq)]
enum Line {
    /// A line, in the buffer, its newline and all.
    Read,
    /// A line longer than the limit, read to its end and dropped.
    TooLong,
}

/// Read a line of at most `limit` bytes into `line`; `None` at the end of
/// the stream. A last line with no newline still counts.
async fn next_line<R: AsyncBufRead + Unpin>(
    read: &mut R,
    line: &mut Vec<u8>,
    limit: usize,
) -> std::io::Result<Option<Line>> {
    line.clear();
    let mut too_long = false;
    let mut any = false;
    loop {
        let available = read.fill_buf().await?;
        if available.is_empty() {
            return Ok(any.then_some(if too_long { Line::TooLong } else { Line::Read }));
        }
        any = true;
        let (taken, ended) = match available.iter().position(|&b| b == b'\n') {
            Some(at) => (at + 1, true),
            None => (available.len(), false),
        };
        if !too_long {
            if line.len() + taken > limit {
                too_long = true;
                *line = Vec::new();
            } else {
                line.extend_from_slice(&available[..taken]);
            }
        }
        read.consume(taken);
        if ended {
            return Ok(Some(if too_long { Line::TooLong } else { Line::Read }));
        }
    }
}

/// Read one message: `None` at the end of the stream, and the reason for
/// a line that is not one, which the caller drops and reads past. An
/// empty line, a keepalive, is read past here.
async fn next_message<R: AsyncBufRead + Unpin, T: for<'de> Deserialize<'de>>(
    read: &mut R,
    line: &mut Vec<u8>,
) -> std::io::Result<Option<Result<T, String>>> {
    loop {
        return Ok(match next_line(read, line, MAX_LINE).await? {
            None => None,
            Some(Line::TooLong) => Some(Err(format!("a line longer than {MAX_LINE} bytes"))),
            Some(Line::Read) if line.trim_ascii().is_empty() => continue,
            Some(Line::Read) => Some(
                serde_json::from_slice(line.trim_ascii_end()).map_err(|error| error.to_string()),
            ),
        });
    }
}

/// A stream's read side that notes when it last gave anything, which is
/// how long the engine has heard nothing from a client.
struct Heard<R> {
    read: R,
    at: Arc<Mutex<Instant>>,
}

impl<R: AsyncRead + Unpin> AsyncRead for Heard<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.read).poll_read(cx, buf);
        if matches!(polled, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            if let Ok(mut at) = self.at.lock() {
                *at = Instant::now();
            }
        }
        polled
    }
}

/// The first message, which has to be one: the handshake has nothing to
/// go on without it.
async fn first_message<R: AsyncBufRead + Unpin, T: for<'de> Deserialize<'de>>(
    read: &mut R,
    line: &mut Vec<u8>,
) -> Result<T, WireError> {
    match next_message(read, line).await? {
        Some(Ok(message)) => Ok(message),
        Some(Err(why)) => Err(WireError::Malformed(why)),
        None => Err(WireError::Closed),
    }
}

/// What this engine tells a client it lets in, once it is connected.
fn welcome(engine: &Engine, connection: u64) -> Welcome {
    Welcome {
        protocol: PROTOCOL,
        engine_version: VERSION.to_string(),
        connection,
        host: host_name(),
        home: perch_github::config::home_dir(),
        data_root: perch_github::config::data_dir().unwrap_or_default(),
        clients: engine.connections().saturating_sub(1),
        runs: engine.active_runs(),
        build: build().to_string(),
    }
}

/// What this engine says of itself to a client it does not let in. The
/// one asking is not connected, so every connection counts.
fn about(engine: &Engine) -> About {
    About {
        engine_version: VERSION.to_string(),
        protocol: PROTOCOL,
        build: build().to_string(),
        runs: engine.active_runs(),
        clients: engine.connections(),
    }
}

/// This process's build: the `<build>` of the `perchd-<build>` it runs as,
/// which the app installed, else its version. Read once, as the engine
/// first says it: a newer build's bridge may remove this one's binary
/// while it runs, and Linux then names it `perchd-<build> (deleted)`.
pub fn build() -> &'static str {
    static BUILD: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BUILD.get_or_init(|| {
        let exe = std::env::current_exe().ok();
        build_named(
            exe.as_deref()
                .and_then(std::path::Path::file_name)
                .and_then(std::ffi::OsStr::to_str),
        )
    })
}

/// The build an executable of `name` is.
fn build_named(name: Option<&str>) -> String {
    name.map(|name| name.strip_suffix(" (deleted)").unwrap_or(name))
        .and_then(|name| name.strip_prefix("perchd-"))
        .filter(|build| provision::plain_word(build))
        .unwrap_or(VERSION)
        .to_string()
}

fn host_name() -> String {
    let mut name = [0u8; 256];
    // SAFETY: the buffer is as long as the length given, and gethostname
    // writes no further.
    let ok = unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) } == 0;
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    ok.then(|| String::from_utf8_lossy(&name[..end]).into_owned())
        .unwrap_or_default()
}

/// The refusal a client of `app_version` speaking `protocol` gets, naming
/// both sides.
fn refusal(protocol: u64, app_version: &str) -> String {
    format!(
        "Perch {app_version} speaks protocol {protocol}, and this engine, Perch {VERSION}, speaks protocol {PROTOCOL}. Update the {}.",
        if protocol < u64::from(PROTOCOL) {
            "app"
        } else {
            "engine"
        },
    )
}

/// What a client's hello asks for.
#[derive(Debug, PartialEq, Eq)]
enum Asked {
    /// To connect, speaking this engine's protocol.
    Connect(Hello),
    /// To stop, whatever its protocol, asked by a client of
    /// `app_version`.
    Stop {
        if_idle: bool,
        unless_build: Option<String>,
        app_version: String,
    },
}

/// The client's hello, or why it is refused (with no `About`, which the
/// caller fills in). Only the protocol and the intent are read before the
/// protocol is checked: a hello from another protocol may be another
/// shape, and its client is still owed the refusal that names both. A
/// stop is read whatever the protocol, as it is what an app of another
/// one sends to make way for its own engine.
fn check_hello(hello: serde_json::Value) -> Result<Asked, WireError> {
    let Some(protocol) = hello.get("protocol").and_then(serde_json::Value::as_u64) else {
        return Err(WireError::Malformed("a hello with no protocol".into()));
    };
    let intent = hello
        .get("intent")
        .and_then(|intent| serde_json::from_value::<Intent>(intent.clone()).ok());
    let app_version = hello.get("app_version").and_then(serde_json::Value::as_str);
    if let Some(Intent::Stop {
        if_idle,
        unless_build,
    }) = intent
    {
        return Ok(Asked::Stop {
            if_idle,
            unless_build,
            app_version: app_version.unwrap_or_default().to_string(),
        });
    }
    if protocol != u64::from(PROTOCOL) {
        let app_version = app_version.unwrap_or("(of no version said)");
        return Err(WireError::Refused(
            refusal(protocol, app_version),
            About::default(),
        ));
    }
    serde_json::from_value(hello)
        .map(Asked::Connect)
        .map_err(|error| WireError::Malformed(error.to_string()))
}

/// The answer to a client of `app_version` asking the engine to stop: one
/// of the build it would start stays, and so does one newer than the
/// client, which never makes way for an older build; only an engine that
/// runs as a service stops for the wire, the app's own never; one with
/// runs or clients is busy when the client asked only if idle.
///
/// The client weighed the engine before it asked, but not as it asked: a
/// restart offered hours ago, or a replace raced by another Mac, can reach
/// an engine that is not the one weighed. Staying, the client joins it and
/// weighs what it finds.
fn answer_stop(
    about: About,
    lifetime: Lifetime,
    if_idle: bool,
    unless_build: Option<&str>,
    app_version: &str,
) -> Answer {
    if unless_build == Some(about.build.as_str()) || newer_than(&about.engine_version, app_version)
    {
        return Answer::Staying(about);
    }
    if lifetime != Lifetime::Service {
        return Answer::Refused {
            reason: "This engine is the Perch app's own, and stops only when the app quits."
                .to_string(),
            about,
        };
    }
    if if_idle && (about.runs > 0 || about.clients > 0) {
        return Answer::Busy(about);
    }
    Answer::Stopping(about)
}

/// Whether `version` is newer than `than`, both semantic versions. Not when
/// either is not one.
fn newer_than(version: &str, than: &str) -> bool {
    match (
        semver::Version::parse(version),
        semver::Version::parse(than),
    ) {
        (Ok(version), Ok(than)) => version > than,
        _ => false,
    }
}

/// Serve one client on `stream` until it hangs up, its stream fails, or
/// it falls more than `MAX_BACKLOG` behind. Its windows close in the engine
/// when it goes; the engine itself goes on, unless it lives only while
/// connected.
pub async fn serve<S>(engine: Engine, stream: S) -> Result<(), WireError>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    serve_with(engine, stream, Limits::default()).await
}

pub(crate) async fn serve_with<S>(
    engine: Engine,
    stream: S,
    limits: Limits,
) -> Result<(), WireError>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let heard = Arc::new(Mutex::new(Instant::now()));
    let mut read = BufReader::new(Heard {
        read,
        at: heard.clone(),
    });
    let mut line = Vec::new();
    // A client that never says hello, an ssh session stuck before the app
    // writes, is given up on like one gone quiet later.
    let hello = tokio::time::timeout(
        limits.idle,
        first_message::<_, serde_json::Value>(&mut read, &mut line),
    )
    .await
    .unwrap_or(Err(WireError::Silent))
    .and_then(check_hello);
    let hello = match hello {
        Ok(Asked::Connect(hello)) => hello,
        Ok(Asked::Stop {
            if_idle,
            unless_build,
            app_version,
        }) => {
            let answer = answer_stop(
                about(&engine),
                engine.lifetime(),
                if_idle,
                unless_build.as_deref(),
                &app_version,
            );
            tracing::info!(?answer, "a client asked the engine to stop");
            write_line(&mut write, &answer).await?;
            let _ = write.shutdown().await;
            if matches!(answer, Answer::Stopping(_)) {
                engine.ask_to_stop();
            }
            return Ok(());
        }
        Err(WireError::Refused(reason, _)) => {
            tracing::warn!(%reason, "refused a client");
            let about = about(&engine);
            write_line(
                &mut write,
                &Answer::Refused {
                    reason: reason.clone(),
                    about: about.clone(),
                },
            )
            .await?;
            let _ = write.shutdown().await;
            return Err(WireError::Refused(reason, about));
        }
        // Nothing said and gone: no fault of the engine's. A bridge waiting
        // for an engine it asked to stop connects to see whether the socket
        // still answers, and an ssh session can end before the app writes.
        Err(WireError::Closed) => {
            tracing::info!("a connection closed before its hello");
            return Err(WireError::Closed);
        }
        Err(error) => {
            tracing::error!(%error, "closing a connection before its hello");
            return Err(error);
        }
    };
    tracing::info!(app = hello.app_version, "client said hello");

    // Connected before the welcome, so a client that has it is already
    // hearing events. The sink never waits: the events are written out by
    // the line, and what is waiting to be written is counted.
    let (events, mut queued) = tokio::sync::mpsc::unbounded_channel::<(WindowId, Event)>();
    let connection = engine.connect(move |window, event| {
        let _ = events.send((window, event));
    });
    write_line(
        &mut write,
        &Answer::Welcome(welcome(&engine, connection.id())),
    )
    .await?;
    // Told when the client cannot be written to any more, or has fallen
    // too far behind: either way it is done.
    let done = Arc::new(tokio::sync::Notify::new());
    let backlog = Arc::new(AtomicUsize::new(0));
    let (lines, mut to_write) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let encoder = tokio::spawn({
        let done = done.clone();
        let backlog = backlog.clone();
        async move {
            while let Some((window, event)) = queued.recv().await {
                let line = match encode(&EventLine { window, event }) {
                    Ok(line) => line,
                    // An event that will not serialize is a bug here, not
                    // the client's; the rest still go.
                    Err(error) => {
                        tracing::error!(%error, "an event would not serialize; dropped");
                        continue;
                    }
                };
                let behind = backlog.fetch_add(line.len(), Ordering::Relaxed) + line.len();
                if behind > limits.backlog {
                    tracing::warn!(behind, "a client fell too far behind; closing it");
                    done.notify_one();
                    return true;
                }
                if lines.send(line).is_err() {
                    return false;
                }
            }
            false
        }
    });
    let mut writer = tokio::spawn({
        let done = done.clone();
        async move {
            while let Some(line) = to_write.recv().await {
                let written = async {
                    write.write_all(&line).await?;
                    write.flush().await
                };
                if let Err(error) = written.await {
                    tracing::info!(%error, "client stopped reading");
                    done.notify_one();
                    return;
                }
                backlog.fetch_sub(line.len(), Ordering::Relaxed);
            }
            let _ = write.shutdown().await;
        }
    });
    // A client that has said nothing, not even a keepalive, in `idle` is
    // asleep or cut off, and is done with as one that hung up.
    let silent = Arc::new(AtomicBool::new(false));
    let watchdog = tokio::spawn({
        let (done, silent) = (done.clone(), silent.clone());
        async move {
            loop {
                let heard = *heard.lock().unwrap_or_else(|e| e.into_inner());
                // An unlimited wait, as the loopback's, never comes due.
                let Some(due) = heard.checked_add(limits.idle) else {
                    return;
                };
                if Instant::now() >= due {
                    tracing::info!(idle = ?limits.idle, "heard nothing from a client; closing it");
                    silent.store(true, Ordering::Relaxed);
                    done.notify_one();
                    return;
                }
                tokio::time::sleep_until(due).await;
            }
        }
    });

    let ended = loop {
        let next = tokio::select! {
            next = next_message::<_, CommandLine>(&mut read, &mut line) => next,
            () = done.notified() => break Ok(()),
        };
        match next {
            Ok(Some(Ok(CommandLine { window, command }))) => {
                if connection.send(window, command).is_err() {
                    break Ok(());
                }
            }
            Ok(Some(Err(why))) => {
                tracing::error!(%why, "a command line that does not parse; dropped")
            }
            Ok(None) => break Ok(()),
            Err(error) => {
                tracing::info!(%error, "closing a connection");
                break Err(WireError::Io(error));
            }
        }
    };
    // The windows close and the sink goes; the encoder ends with what is
    // left, and the writer writes it if the client is still reading.
    watchdog.abort();
    drop(connection);
    let behind = encoder.await.unwrap_or(false);
    if tokio::time::timeout(limits.last_writes, &mut writer)
        .await
        .is_err()
    {
        writer.abort();
    }
    if behind {
        return Err(WireError::Behind);
    }
    if silent.load(Ordering::Relaxed) {
        return Err(WireError::Silent);
    }
    ended
}

/// Connect to an engine on `stream`: the handshake, then a handle and the
/// events, as `backend::spawn` gives them, with the engine's welcome.
/// Must run on a tokio runtime, which carries the connection after.
pub async fn connect<S>(
    stream: S,
) -> Result<
    (
        Backend,
        std::sync::mpsc::Receiver<(WindowId, Event)>,
        Welcome,
    ),
    WireError,
>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (welcome, session) = handshake(stream).await?;
    let (commands, outgoing) = tokio::sync::mpsc::unbounded_channel::<(WindowId, Command)>();
    let (backend, navigations) = Backend::over_wire(commands, welcome.connection);
    let (events, received) = std::sync::mpsc::channel();
    session.carry(outgoing, navigations, events);
    Ok((backend, received, welcome))
}

/// A connection past its handshake, not yet carrying anything.
pub(crate) struct Session<S> {
    read: BufReader<tokio::io::ReadHalf<S>>,
    write: tokio::io::WriteHalf<S>,
    line: Vec<u8>,
    /// How often to say it is still there: `KEEPALIVE`, but for a test.
    pub(crate) keepalive: Duration,
}

/// Say hello on `stream` and hear the engine's answer.
pub(crate) async fn handshake<S>(stream: S) -> Result<(Welcome, Session<S>), WireError>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let mut line = Vec::new();
    write_line(&mut write, &Hello::connect()).await?;
    // A login shell on the far side of ssh may say something of its own
    // before the bridge starts, a profile's greeting say: it is passed
    // over, up to a point. What it said with no newline after, a terminal
    // title say, is on the answer's line, before it.
    let mut said = 0;
    let answer = loop {
        match next_message::<_, Answer>(&mut read, &mut line).await? {
            Some(Ok(answer)) => break answer,
            Some(Err(why)) => {
                if let Some(answer) = answer_within(&line) {
                    break answer;
                }
                if said >= MAX_SAID_FIRST {
                    return Err(WireError::Malformed(why));
                }
                said += 1;
                tracing::info!(
                    line = %String::from_utf8_lossy(line.trim_ascii_end()),
                    "passed over a line before the engine's answer"
                );
            }
            None => return Err(WireError::Closed),
        }
    };
    match answer {
        Answer::Welcome(welcome) => Ok((
            welcome,
            Session {
                read,
                write,
                line,
                keepalive: KEEPALIVE,
            },
        )),
        Answer::Refused { reason, about } => Err(WireError::Refused(reason, about)),
        // Not what a hello asking to connect is answered with.
        other => Err(WireError::Malformed(format!(
            "{other:?} in answer to a hello asking to connect"
        ))),
    }
}

/// Ask the engine on `stream` to stop, unless it is of `unless_build`,
/// and give its answer: what `perchd bridge --replace` sends.
pub async fn ask_to_stop<S>(
    stream: S,
    if_idle: bool,
    unless_build: Option<String>,
) -> Result<Answer, WireError>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let mut line = Vec::new();
    let hello = Hello {
        intent: Intent::Stop {
            if_idle,
            unless_build,
        },
        ..Hello::connect()
    };
    write_line(&mut write, &hello).await?;
    first_message(&mut read, &mut line).await
}

/// How many lines before the engine's answer a client passes over.
const MAX_SAID_FIRST: usize = 64;

/// The engine's answer at the end of `line`, after something else.
fn answer_within(line: &[u8]) -> Option<Answer> {
    let line = line.trim_ascii_end();
    (1..line.len())
        .filter(|&at| line[at..].starts_with(b"{\""))
        .find_map(|at| serde_json::from_slice(&line[at..]).ok())
}

impl<S> Session<S>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    /// Ask for everything, as a client does once connected, and read what
    /// comes until the host's checks do: `perch --connect`. `None` when
    /// they don't come within `within`, from an engine that only now
    /// started and is still looking, or one that is not a host's and never
    /// looks.
    pub(crate) async fn host_checks(
        mut self,
        within: Duration,
    ) -> Option<Vec<crate::host::HostCheck>> {
        let asked = CommandLine {
            window: crate::backend::NO_WINDOW,
            command: Command::Resync,
        };
        write_line(&mut self.write, &asked).await.ok()?;
        let read = async {
            loop {
                match next_message::<_, EventLine>(&mut self.read, &mut self.line).await {
                    Ok(Some(Ok(EventLine {
                        event: Event::HostChecks(checks),
                        ..
                    }))) => return Some(checks),
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => return None,
                }
            }
        };
        tokio::time::timeout(within, read).await.ok().flatten()
    }

    /// Write what comes from `outgoing` and hand what the engine says,
    /// past `navigations`, to `events`, on tasks of their own. The
    /// returned receiver resolves, with an error, once the engine has
    /// hung up or its stream has failed, or every sender to `outgoing`
    /// has gone.
    pub(crate) fn carry(
        self,
        mut outgoing: tokio::sync::mpsc::UnboundedReceiver<(WindowId, Command)>,
        navigations: Arc<crate::backend::client::Navigations>,
        events: std::sync::mpsc::Sender<(WindowId, Event)>,
    ) -> tokio::sync::oneshot::Receiver<()> {
        let Session {
            mut read,
            mut write,
            mut line,
            keepalive,
        } = self;
        // Told when every handle has gone. The reader stops then too: the two
        // halves share the stream, which closes only when both have let go of
        // it, and a stream whose shutdown does nothing, as a child's stdin's
        // does, would otherwise stay open until the engine next said something.
        let hung_up = Arc::new(tokio::sync::Notify::new());
        // Ends when every handle has gone, which hangs up. Says it is still
        // there every `keepalive` it has said nothing else, so the engine
        // can tell a quiet client from one that has gone.
        tokio::spawn({
            let hung_up = hung_up.clone();
            async move {
                let mut quiet = tokio::time::interval_at(Instant::now() + keepalive, keepalive);
                quiet.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    let (window, command) = tokio::select! {
                        next = outgoing.recv() => match next {
                            Some(next) => next,
                            None => break,
                        },
                        _ = quiet.tick() => {
                            let said = async {
                                write.write_all(b"\n").await?;
                                write.flush().await
                            };
                            if let Err(error) = said.await {
                                tracing::info!(%error, "engine stopped reading");
                                return;
                            }
                            continue;
                        }
                    };
                    quiet.reset();
                    match write_line(&mut write, &CommandLine { window, command }).await {
                        Ok(()) => {}
                        Err(WireError::Malformed(error)) => {
                            tracing::error!(%error, "a command would not serialize; dropped")
                        }
                        Err(error) => {
                            tracing::info!(%error, "engine stopped reading");
                            return;
                        }
                    }
                }
                let _ = write.shutdown().await;
                drop(write);
                hung_up.notify_one();
            }
        });
        // Dropped as the reader ends, which is what the caller waits on.
        let (ended, ending) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ended = ended;
            loop {
                let next = tokio::select! {
                    next = next_message::<_, EventLine>(&mut read, &mut line) => next,
                    () = hung_up.notified() => return,
                };
                match next {
                    Ok(Some(Ok(EventLine { window, event }))) => {
                        if navigations.pass(window, &event) && events.send((window, event)).is_err()
                        {
                            return;
                        }
                    }
                    Ok(Some(Err(why))) => {
                        tracing::error!(%why, "an event line that does not parse; dropped")
                    }
                    Ok(None) => {
                        tracing::info!("engine hung up");
                        return;
                    }
                    Err(error) => {
                        tracing::error!(%error, "closing the connection to the engine");
                        return;
                    }
                }
            }
        });
        ending
    }
}

/// The app's engine in this process, reached through the wire over a pipe
/// in memory rather than over channels: `PERCH_WIRE_LOOPBACK=1`. Every
/// command and event goes through JSON and back, so the Mac shows what a
/// change to the wire form breaks without a host. Like `spawn`'s, the
/// engine lives while the app's connection does.
pub fn loopback(
    check: CheckConfig,
) -> std::io::Result<(Backend, std::sync::mpsc::Receiver<(WindowId, Event)>)> {
    let unstarted = Engine::build(Lifetime::WhileConnected)?;
    let engine = unstarted.engine().clone();
    let runtime = engine.runtime().clone();
    let (near, far) = tokio::io::duplex(1 << 16);
    // No network to lose, and the app's keepalives share a process with
    // the watchdog: an App Nap that held both off would otherwise end the
    // engine, which lives only while this connection does.
    let limits = Limits {
        idle: Duration::MAX,
        ..Limits::default()
    };
    runtime.spawn(async move {
        if let Err(error) = serve_with(engine, far, limits).await {
            tracing::error!(%error, "the loopback connection ended");
        }
    });
    let (backend, events, welcome) = runtime
        .block_on(connect(near))
        .map_err(std::io::Error::other)?;
    tracing::info!(host = welcome.host, "engine over the loopback wire");
    // Connected before it runs, so the app hears its startup as it would
    // in process, and has no need to ask for it again.
    unstarted.start(check);
    Ok((backend, events))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn lines(input: &[u8], limit: usize) -> Vec<(Line, String)> {
        let mut read = BufReader::with_capacity(4, input);
        let mut line = Vec::new();
        let mut read_lines = Vec::new();
        while let Some(what) = next_line(&mut read, &mut line, limit).await.unwrap() {
            read_lines.push((what, String::from_utf8_lossy(&line).into_owned()));
        }
        read_lines
    }

    #[tokio::test]
    async fn a_line_past_the_limit_is_read_to_its_end_and_dropped() {
        assert_eq!(
            lines(b"short\nfar too long a line\nok\nlast", 8).await,
            vec![
                (Line::Read, "short\n".into()),
                (Line::TooLong, String::new()),
                (Line::Read, "ok\n".into()),
                (Line::Read, "last".into()),
            ]
        );
        assert!(lines(b"", 8).await.is_empty());
    }

    #[test]
    fn a_hello_is_checked_by_its_protocol_before_its_shape() {
        let refused = |hello: serde_json::Value| match check_hello(hello) {
            Err(WireError::Refused(reason, _)) => reason,
            other => panic!("{other:?}"),
        };
        let reason = refused(serde_json::json!({
            "protocol": PROTOCOL + 1,
            "app_version": { "major": 99 },
        }));
        assert!(
            reason.contains(&format!("protocol {}", PROTOCOL + 1)),
            "{reason}"
        );
        assert!(reason.contains("Update the engine"), "{reason}");
        assert!(matches!(
            check_hello(serde_json::json!({ "app_version": "1" })),
            Err(WireError::Malformed(_))
        ));
        let Asked::Connect(hello) = check_hello(serde_json::json!({
            "protocol": PROTOCOL,
            "app_version": "1.2.3",
        }))
        .unwrap() else {
            panic!("not a connect");
        };
        assert_eq!(hello.app_version, "1.2.3");
        assert_eq!(hello.intent, Intent::Connect);
    }

    /// A stop is heard whatever protocol asks it, past or future: it is
    /// how an updated app makes way for its own engine.
    #[test]
    fn a_stop_is_read_before_the_protocol_is_checked() {
        for protocol in [PROTOCOL, PROTOCOL + 7] {
            let asked = check_hello(serde_json::json!({
                "protocol": protocol,
                "app_version": "9.9.9",
                "intent": { "Stop": { "if_idle": true, "unless_build": "9.9.9", "said": "later" } },
                "added_later": [1, 2],
            }))
            .unwrap();
            assert_eq!(
                asked,
                Asked::Stop {
                    if_idle: true,
                    unless_build: Some("9.9.9".into()),
                    app_version: "9.9.9".into(),
                }
            );
        }
        // An intent of a later protocol that this one does not know is
        // refused as that protocol is.
        assert!(matches!(
            check_hello(serde_json::json!({
                "protocol": PROTOCOL + 1,
                "app_version": "9.9.9",
                "intent": "Wave",
            })),
            Err(WireError::Refused(..))
        ));
    }

    #[test]
    fn the_handshake_s_old_shapes_still_read() {
        // A hello from before protocol 5 connects.
        let hello: Hello = serde_json::from_str(r#"{"protocol":4,"app_version":"0.8.0"}"#).unwrap();
        assert_eq!(hello.intent, Intent::Connect);
        let stop: Intent = serde_json::from_str(r#"{"Stop":{"if_idle":false}}"#).unwrap();
        assert_eq!(
            stop,
            Intent::Stop {
                if_idle: false,
                unless_build: None
            }
        );
        // A refusal from an engine before protocol 5 says only why.
        let refused: Answer = serde_json::from_str(r#"{"Refused":{"reason":"no"}}"#).unwrap();
        assert_eq!(
            refused,
            Answer::Refused {
                reason: "no".into(),
                about: About::default()
            }
        );
        // A welcome with no build, and one with fields from later.
        let welcome: Answer = serde_json::from_str(
            r#"{"Welcome":{"protocol":4,"engine_version":"0.8.0","connection":3,"host":"halo","home":"/home/c","data_root":"/home/c/.perch","clients":1,"runs":2,"later":true}}"#,
        )
        .unwrap();
        let Answer::Welcome(welcome) = welcome else {
            panic!("{welcome:?}");
        };
        assert_eq!(welcome.build, "");
        assert_eq!((welcome.clients, welcome.runs), (1, 2));
    }

    /// `perch --connect` asks for everything once in, and reads past what
    /// else comes to the host's checks; an engine that never sends them
    /// is given up on.
    #[tokio::test]
    async fn the_host_checks_are_read_out_of_a_resync() {
        let (near, far) = tokio::io::duplex(1 << 16);
        let engine = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(far);
            let mut read = BufReader::new(read);
            let mut line = Vec::new();
            let hello = next_message::<_, Hello>(&mut read, &mut line).await;
            assert!(matches!(hello, Ok(Some(Ok(_)))));
            let welcome = Answer::Welcome(Welcome {
                protocol: PROTOCOL,
                engine_version: VERSION.into(),
                connection: 1,
                host: "halo".into(),
                home: "/home/c".into(),
                data_root: "/home/c/.perch".into(),
                clients: 0,
                runs: 0,
                build: VERSION.into(),
            });
            write_line(&mut write, &welcome).await.unwrap();
            let asked = next_message::<_, CommandLine>(&mut read, &mut line).await;
            let Ok(Some(Ok(CommandLine {
                command: Command::Resync,
                ..
            }))) = asked
            else {
                panic!("not a resync: {asked:?}");
            };
            let check = crate::host::HostCheck {
                what: "git".into(),
                ok: true,
                required: true,
                detail: "git version 2.43.0.".into(),
                fix: None,
            };
            for event in [
                Event::Notice("hello".into()),
                Event::HostChecks(vec![check.clone()]),
            ] {
                let line = EventLine {
                    window: crate::backend::NO_WINDOW,
                    event,
                };
                write_line(&mut write, &line).await.unwrap();
            }
            // Held open, as an engine would be.
            let _ = next_message::<_, CommandLine>(&mut read, &mut line).await;
            check
        });
        let (_, session) = handshake(near).await.unwrap();
        let checks = session.host_checks(Duration::from_secs(5)).await;
        let check = engine.await.unwrap();
        assert_eq!(checks, Some(vec![check]));

        // An engine that says nothing more.
        let (near, far) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(far);
            let mut read = BufReader::new(read);
            let mut line = Vec::new();
            let _ = next_message::<_, Hello>(&mut read, &mut line).await;
            let welcome = Answer::Welcome(Welcome {
                protocol: PROTOCOL,
                engine_version: VERSION.into(),
                connection: 1,
                host: "halo".into(),
                home: "/home/c".into(),
                data_root: "/home/c/.perch".into(),
                clients: 0,
                runs: 0,
                build: VERSION.into(),
            });
            write_line(&mut write, &welcome).await.unwrap();
            let _ = next_message::<_, CommandLine>(&mut read, &mut line).await;
            let _ = next_message::<_, CommandLine>(&mut read, &mut line).await;
        });
        let (_, session) = handshake(near).await.unwrap();
        assert_eq!(session.host_checks(Duration::from_millis(200)).await, None);
    }

    #[test]
    fn the_handshake_s_new_shapes_go_and_come_back() {
        let about = About {
            engine_version: "0.9.0".into(),
            protocol: PROTOCOL,
            build: "0.9.0-0123456789ab".into(),
            runs: 2,
            clients: 1,
        };
        let answers = [
            Answer::Refused {
                reason: "no".into(),
                about: about.clone(),
            },
            Answer::Busy(about.clone()),
            Answer::Stopping(about.clone()),
            Answer::Staying(about.clone()),
            Answer::Welcome(Welcome {
                protocol: PROTOCOL,
                engine_version: "0.9.0".into(),
                connection: 1,
                host: "halo".into(),
                home: "/home/c".into(),
                data_root: "/home/c/.perch".into(),
                clients: 0,
                runs: 0,
                build: "0.9.0".into(),
            }),
        ];
        for answer in answers {
            let line = serde_json::to_string(&answer).unwrap();
            assert_eq!(serde_json::from_str::<Answer>(&line).unwrap(), answer);
        }
        // The refusal's fields sit beside its reason, as an older client
        // reads it: the reason, and nothing it does not know.
        let line = serde_json::to_value(Answer::Refused {
            reason: "no".into(),
            about,
        })
        .unwrap();
        assert_eq!(line["Refused"]["reason"], "no");
        assert_eq!(line["Refused"]["build"], "0.9.0-0123456789ab");
        let hello = Hello {
            intent: Intent::Stop {
                if_idle: true,
                unless_build: Some("0.9.0".into()),
            },
            ..Hello::connect()
        };
        let line = serde_json::to_string(&hello).unwrap();
        assert_eq!(serde_json::from_str::<Hello>(&line).unwrap(), hello);
    }

    #[test]
    fn an_engine_stops_for_the_wire_only_as_a_service_and_idle_when_asked() {
        let about = |runs, clients| About {
            engine_version: VERSION.into(),
            protocol: PROTOCOL,
            build: "0.9.0".into(),
            runs,
            clients,
        };
        let stop = |runs, clients, lifetime, if_idle, unless: Option<&str>| {
            answer_stop(about(runs, clients), lifetime, if_idle, unless, VERSION)
        };
        assert!(matches!(
            stop(0, 0, Lifetime::Service, true, Some("0.9.1")),
            Answer::Stopping(_)
        ));
        assert!(matches!(
            stop(1, 0, Lifetime::Service, true, None),
            Answer::Busy(About { runs: 1, .. })
        ));
        assert!(matches!(
            stop(0, 2, Lifetime::Service, true, None),
            Answer::Busy(About { clients: 2, .. })
        ));
        assert!(matches!(
            stop(3, 2, Lifetime::Service, false, None),
            Answer::Stopping(_)
        ));
        // The build the client would start stays, busy or not.
        assert!(matches!(
            stop(3, 2, Lifetime::Service, false, Some("0.9.0")),
            Answer::Staying(_)
        ));
        // The app's own engine never stops for the wire.
        assert!(matches!(
            stop(0, 0, Lifetime::WhileConnected, false, None),
            Answer::Refused { .. }
        ));
        // Nor does one newer than the client asking, whatever it was told
        // of the engine before: it stays, busy or idle, and the client
        // joins it. One of an older version, the same, or one that names
        // none, stops as asked.
        let asked_by = |version: &str, if_idle| {
            let mut newer = about(3, 1);
            newer.engine_version = "0.10.0".into();
            answer_stop(
                newer,
                Lifetime::Service,
                if_idle,
                Some("0.9.0-0123456789ab"),
                version,
            )
        };
        assert!(matches!(asked_by("0.9.0", false), Answer::Staying(_)));
        assert!(matches!(asked_by("0.9.0", true), Answer::Staying(_)));
        assert!(matches!(asked_by("0.10.0", false), Answer::Stopping(_)));
        assert!(matches!(asked_by("0.11.0", false), Answer::Stopping(_)));
        assert!(matches!(asked_by("0.11.0", true), Answer::Busy(_)));
        assert!(matches!(asked_by("", false), Answer::Stopping(_)));
    }

    #[test]
    fn a_build_is_read_from_the_name_the_app_installed_it_by() {
        assert_eq!(build_named(Some("perchd-0.9.0")), "0.9.0");
        assert_eq!(
            build_named(Some("perchd-0.9.0-0123456789ab (deleted)")),
            "0.9.0-0123456789ab"
        );
        assert_eq!(build_named(Some("perchd")), VERSION);
        assert_eq!(build_named(Some("perch")), VERSION);
        assert_eq!(build_named(Some("perchd-")), VERSION);
        assert_eq!(build_named(Some("perchd-0.9;x")), VERSION);
        assert_eq!(build_named(None), VERSION);
    }

    /// What a login shell says before the bridge starts is passed over,
    /// a terminal title with no newline after it on the answer's line too.
    #[tokio::test]
    async fn the_answer_is_found_after_what_a_login_shell_said() {
        let refused = encode(&Answer::Refused {
            reason: "no".into(),
            about: About::default(),
        })
        .unwrap();
        let mut far_says = b"Welcome to halo\n\x1b]0;chris@halo\x07".to_vec();
        far_says.extend_from_slice(&refused);
        let (near, mut far) = tokio::io::duplex(1 << 16);
        far.write_all(&far_says).await.unwrap();
        match handshake(near).await {
            Err(WireError::Refused(reason, _)) => assert_eq!(reason, "no"),
            Err(other) => panic!("{other:?}"),
            Ok(_) => panic!("welcomed"),
        }
    }
}
