//! The engine over a byte stream. One JSON message a line, on any duplex
//! stream: a socket, ssh's stdio, or a pipe in memory.
//!
//! The client sends a `Hello`; the engine answers `Welcome`, or `Refused`
//! when the protocols differ, before any command runs. Then the client
//! sends `{ window, command }` lines and the engine `{ window, event }`
//! lines, the window ids the client's own (see `backend::connections`).
//! A line that does not parse closes its connection, with the reason in
//! the log; logs never go on the stream.
//!
//! The types and their wire form live in this crate together so they
//! cannot drift apart: what goes over is `Command` and `Event` as serde
//! writes them.

pub mod bytes;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::backend::{Backend, CheckConfig, Command, Engine, Event, Lifetime, WindowId};

/// The wire's version. A change to `Command` or `Event` that an older
/// peer would read wrong, or not at all, moves it on.
pub const PROTOCOL: u32 = 1;

/// The version of Perch this is, as each side names itself.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    /// The engine's machine, by its host name.
    pub host: String,
    /// The engine's home directory: what `~` means in its paths.
    pub home: PathBuf,
    /// Where the engine keeps its checkouts and runs.
    pub data_root: PathBuf,
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
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Io(error) => write!(f, "{error}"),
            WireError::Malformed(why) => write!(f, "a malformed line: {why}"),
            WireError::Closed => f.write_str("the connection closed during the handshake"),
            WireError::Refused(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for WireError {}

impl From<std::io::Error> for WireError {
    fn from(error: std::io::Error) -> Self {
        WireError::Io(error)
    }
}

/// Write one message and its newline.
async fn write_line<W: AsyncWrite + Unpin>(
    write: &mut W,
    message: &impl Serialize,
) -> Result<(), WireError> {
    let mut line =
        serde_json::to_vec(message).map_err(|error| WireError::Malformed(error.to_string()))?;
    line.push(b'\n');
    write.write_all(&line).await?;
    write.flush().await?;
    Ok(())
}

/// Read one message; `None` at the end of the stream.
async fn read_line<R: AsyncBufReadExt + Unpin, T: for<'de> Deserialize<'de>>(
    read: &mut R,
    line: &mut String,
) -> Result<Option<T>, WireError> {
    line.clear();
    let read_bytes = read.read_line(line).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidData {
            WireError::Malformed(error.to_string())
        } else {
            WireError::Io(error)
        }
    })?;
    if read_bytes == 0 {
        return Ok(None);
    }
    serde_json::from_str(line.trim_end())
        .map(Some)
        .map_err(|error| WireError::Malformed(error.to_string()))
}

/// What this engine tells a client it lets in.
fn welcome() -> Welcome {
    Welcome {
        protocol: PROTOCOL,
        engine_version: VERSION.to_string(),
        host: host_name(),
        home: perch_github::config::home_dir(),
        data_root: perch_github::config::data_dir().unwrap_or_default(),
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

/// The refusal a client speaking `hello.protocol` gets, naming both sides.
fn refusal(hello: &Hello) -> String {
    format!(
        "Perch {} speaks protocol {}, and this engine, Perch {VERSION}, speaks protocol {PROTOCOL}. Update the {}.",
        hello.app_version,
        hello.protocol,
        if hello.protocol < PROTOCOL {
            "app"
        } else {
            "engine"
        },
    )
}

/// Serve one client on `stream` until it hangs up or sends a line that
/// does not parse. Its windows close in the engine when it goes; the
/// engine itself goes on, unless it lives only while connected.
pub async fn serve<S>(engine: Engine, stream: S) -> Result<(), WireError>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let mut line = String::new();
    let hello: Hello = match read_line(&mut read, &mut line).await {
        Ok(Some(hello)) => hello,
        Ok(None) => return Err(WireError::Closed),
        Err(error) => {
            tracing::error!(%error, "closing a connection before its hello");
            return Err(error);
        }
    };
    if hello.protocol != PROTOCOL {
        let reason = refusal(&hello);
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
    tracing::info!(app = hello.app_version, "client said hello");

    // Connected before the welcome, so a client that has it is already
    // hearing events.
    let (events, mut outgoing) = tokio::sync::mpsc::unbounded_channel::<(WindowId, Event)>();
    let connection = engine.connect(move |window, event| {
        let _ = events.send((window, event));
    });
    write_line(&mut write, &Answer::Welcome(welcome())).await?;
    let writer = tokio::spawn(async move {
        while let Some((window, event)) = outgoing.recv().await {
            match write_line(&mut write, &EventLine { window, event }).await {
                Ok(()) => {}
                // An event that will not serialize is a bug here, not the
                // client's; the rest still go.
                Err(WireError::Malformed(error)) => {
                    tracing::error!(%error, "an event would not serialize; dropped")
                }
                Err(error) => {
                    tracing::info!(%error, "client stopped reading");
                    return;
                }
            }
        }
        let _ = write.shutdown().await;
    });

    let ended = loop {
        match read_line::<_, CommandLine>(&mut read, &mut line).await {
            Ok(Some(CommandLine { window, command })) => {
                if connection.send(window, command).is_err() {
                    break Ok(());
                }
            }
            Ok(None) => break Ok(()),
            Err(error) => {
                tracing::error!(%error, "closing a connection");
                break Err(error);
            }
        }
    };
    // The windows close, the sink goes, and the writer sends what is left.
    drop(connection);
    let _ = writer.await;
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
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let mut line = String::new();
    write_line(
        &mut write,
        &Hello {
            protocol: PROTOCOL,
            app_version: VERSION.to_string(),
        },
    )
    .await?;
    let welcome = match read_line::<_, Answer>(&mut read, &mut line).await? {
        Some(Answer::Welcome(welcome)) => welcome,
        Some(Answer::Refused { reason }) => return Err(WireError::Refused(reason)),
        None => return Err(WireError::Closed),
    };

    let (commands, mut outgoing) = tokio::sync::mpsc::unbounded_channel::<(WindowId, Command)>();
    let (backend, navigations) = Backend::over_wire(commands);
    let (events, received) = std::sync::mpsc::channel();
    // Ends when every handle has gone, which hangs up.
    tokio::spawn(async move {
        while let Some((window, command)) = outgoing.recv().await {
            if let Err(error) = write_line(&mut write, &CommandLine { window, command }).await {
                tracing::info!(%error, "engine stopped reading");
                return;
            }
        }
        let _ = write.shutdown().await;
    });
    tokio::spawn(async move {
        loop {
            match read_line::<_, EventLine>(&mut read, &mut line).await {
                Ok(Some(EventLine { window, event })) => {
                    navigations.observe(window, &event);
                    if events.send((window, event)).is_err() {
                        return;
                    }
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
    Ok((backend, received, welcome))
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
    // in process; then asks again, as a client of a running engine does.
    unstarted.start(check);
    backend.resync();
    Ok((backend, events))
}
