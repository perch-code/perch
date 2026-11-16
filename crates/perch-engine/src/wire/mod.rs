//! The engine over a byte stream. One JSON message a line, on any duplex
//! stream: a socket, ssh's stdio, or a pipe in memory.
//!
//! The client sends a `Hello`; the engine answers `Welcome`, or `Refused`
//! when the protocols differ, before any command runs. Then the client
//! sends `{ window, command }` lines and the engine `{ window, event }`
//! lines, the window ids the client's own (see `backend::connections`).
//! A line that does not parse is dropped, with the reason in the log, and
//! the lines after it still go: framing is by the line. Only a handshake
//! that fails, or the stream itself, ends a connection. Logs never go on
//! the stream.
//!
//! The types and their wire form live in this crate together so they
//! cannot drift apart: what goes over is `Command` and `Event` as serde
//! writes them.

pub mod bytes;
pub mod pairs;
pub mod remote;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::backend::{Backend, CheckConfig, Command, Engine, Event, Lifetime, WindowId};

/// The wire's version. A change to `Command` or `Event` that an older
/// peer would read wrong, or not at all, moves it on. 2: `Welcome` counts
/// the engine's other clients and its runs. 3: scope picks carry the
/// handle's ids, a resync's runs and kept settings replace a window's.
pub const PROTOCOL: u32 = 3;

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

/// How far a client may fall behind, and how long its last writes get.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) backlog: usize,
    pub(crate) last_writes: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            backlog: MAX_BACKLOG,
            last_writes: LAST_WRITES,
        }
    }
}

/// The client's first line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub app_version: String,
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
}

/// The engine's answer to a `Hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    Welcome(Welcome),
    Refused { reason: String },
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
    /// The engine would not let the client in, and why.
    Refused(String),
    /// The client fell more than `MAX_BACKLOG` behind.
    Behind,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Io(error) => write!(f, "{error}"),
            WireError::Malformed(why) => write!(f, "a malformed line: {why}"),
            WireError::Closed => f.write_str("the connection closed during the handshake"),
            WireError::Refused(reason) => f.write_str(reason),
            WireError::Behind => f.write_str("the client fell too far behind"),
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
/// a line that is not one, which the caller drops and reads past.
async fn next_message<R: AsyncBufRead + Unpin, T: for<'de> Deserialize<'de>>(
    read: &mut R,
    line: &mut Vec<u8>,
) -> std::io::Result<Option<Result<T, String>>> {
    Ok(match next_line(read, line, MAX_LINE).await? {
        None => None,
        Some(Line::TooLong) => Some(Err(format!("a line longer than {MAX_LINE} bytes"))),
        Some(Line::Read) => {
            Some(serde_json::from_slice(line.trim_ascii_end()).map_err(|error| error.to_string()))
        }
    })
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
    }
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

/// The client's hello, or why it is refused. Only the protocol is read
/// before it is checked: a hello from another protocol may be another
/// shape, and its client is still owed the refusal that names both.
fn check_hello(hello: serde_json::Value) -> Result<Hello, WireError> {
    let Some(protocol) = hello.get("protocol").and_then(serde_json::Value::as_u64) else {
        return Err(WireError::Malformed("a hello with no protocol".into()));
    };
    if protocol != u64::from(PROTOCOL) {
        let app_version = hello
            .get("app_version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("(of no version said)");
        return Err(WireError::Refused(refusal(protocol, app_version)));
    }
    serde_json::from_value(hello).map_err(|error| WireError::Malformed(error.to_string()))
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
    let mut read = BufReader::new(read);
    let mut line = Vec::new();
    let hello = first_message::<_, serde_json::Value>(&mut read, &mut line)
        .await
        .and_then(check_hello);
    let hello = match hello {
        Ok(hello) => hello,
        Err(WireError::Refused(reason)) => {
            tracing::warn!(%reason, "refused a client");
            write_line(
                &mut write,
                &Answer::Refused {
                    reason: reason.clone(),
                },
            )
            .await?;
            let _ = write.shutdown().await;
            return Err(WireError::Refused(reason));
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
}

/// Say hello on `stream` and hear the engine's answer.
pub(crate) async fn handshake<S>(stream: S) -> Result<(Welcome, Session<S>), WireError>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let mut line = Vec::new();
    write_line(
        &mut write,
        &Hello {
            protocol: PROTOCOL,
            app_version: VERSION.to_string(),
        },
    )
    .await?;
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
        Answer::Welcome(welcome) => Ok((welcome, Session { read, write, line })),
        Answer::Refused { reason } => Err(WireError::Refused(reason)),
    }
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
        } = self;
        // Told when every handle has gone. The reader stops then too: the two
        // halves share the stream, which closes only when both have let go of
        // it, and a stream whose shutdown does nothing, as a child's stdin's
        // does, would otherwise stay open until the engine next said something.
        let hung_up = Arc::new(tokio::sync::Notify::new());
        // Ends when every handle has gone, which hangs up.
        tokio::spawn({
            let hung_up = hung_up.clone();
            async move {
                while let Some((window, command)) = outgoing.recv().await {
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
    runtime.spawn(async move {
        if let Err(error) = serve(engine, far).await {
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
            Err(WireError::Refused(reason)) => reason,
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
        let hello = check_hello(serde_json::json!({
            "protocol": PROTOCOL,
            "app_version": "1.2.3",
        }))
        .unwrap();
        assert_eq!(hello.app_version, "1.2.3");
    }

    /// What a login shell says before the bridge starts is passed over,
    /// a terminal title with no newline after it on the answer's line too.
    #[tokio::test]
    async fn the_answer_is_found_after_what_a_login_shell_said() {
        let refused = encode(&Answer::Refused {
            reason: "no".into(),
        })
        .unwrap();
        let mut far_says = b"Welcome to halo\n\x1b]0;chris@halo\x07".to_vec();
        far_says.extend_from_slice(&refused);
        let (near, mut far) = tokio::io::duplex(1 << 16);
        far.write_all(&far_says).await.unwrap();
        match handshake(near).await {
            Err(WireError::Refused(reason)) => assert_eq!(reason, "no"),
            Err(other) => panic!("{other:?}"),
            Ok(_) => panic!("welcomed"),
        }
    }
}
