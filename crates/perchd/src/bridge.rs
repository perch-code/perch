//! `perchd bridge`: what `ssh <host> perchd bridge` runs. Its stdin and
//! stdout become a connection to the engine's socket, byte for byte, until
//! either side closes. Nothing else is ever written to stdout: the app
//! reads it as the wire. What goes wrong goes to stderr.
//!
//! With no engine answering, the bridge starts `perchd serve` in a session
//! of its own, so the engine outlives the ssh session that started it, and
//! waits for its socket.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use perch_engine::lock::EngineLock;

use crate::socket;

/// How long an engine started here gets to answer.
const STARTING: Duration = Duration::from_secs(10);

/// How often the socket is tried while it starts.
const POLL: Duration = Duration::from_millis(25);

pub fn run() -> ExitCode {
    let socket = socket::path();
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(_) => match start_and_connect(&socket) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("perchd bridge: {error}");
                return ExitCode::FAILURE;
            }
        },
    };
    join(stream)
}

/// Copy stdin to the engine and the engine to stdout, until the engine
/// hangs up or stdout closes. Stdin closing only closes the way in: the
/// engine still has its last events to send, and hangs up after.
fn join(stream: UnixStream) -> ExitCode {
    let Ok(mut to_engine) = stream.try_clone() else {
        eprintln!("perchd bridge: could not share the socket between directions");
        return ExitCode::FAILURE;
    };
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut to_engine);
        let _ = to_engine.shutdown(std::net::Shutdown::Write);
    });
    let mut from_engine = stream;
    let mut stdout = std::io::stdout().lock();
    let mut buffer = vec![0u8; 64 << 10];
    loop {
        let read = match from_engine.read(&mut buffer) {
            Ok(0) => return ExitCode::SUCCESS,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("perchd bridge: the engine's connection failed: {error}");
                return ExitCode::FAILURE;
            }
        };
        // Flushed each time: a line held in a buffer is an event late.
        if stdout
            .write_all(&buffer[..read])
            .and_then(|()| stdout.flush())
            .is_err()
        {
            // The client has gone.
            return ExitCode::SUCCESS;
        }
    }
}

/// Start an engine and connect to it once it answers. One that exits
/// without answering was turned away from the home, most likely by an
/// engine still stopping, or racing another bridge's: it is tried again
/// until the time is up.
fn start_and_connect(socket: &Path) -> Result<UnixStream, String> {
    let deadline = Instant::now() + STARTING;
    let mut child: Option<Child> = None;
    let mut last_start = None;
    loop {
        if let Ok(stream) = UnixStream::connect(socket) {
            return Ok(stream);
        }
        let running = match child.as_mut().map(Child::try_wait) {
            Some(Ok(None)) => true,
            Some(Ok(Some(_)) | Err(_)) | None => false,
        };
        let may_start =
            last_start.is_none_or(|at: Instant| at.elapsed() >= Duration::from_millis(500));
        if !running && may_start {
            child = Some(start()?);
            last_start = Some(Instant::now());
        }
        if Instant::now() >= deadline {
            let home = perch_github::config::perch_home();
            let holder = match EngineLock::holder(&home) {
                Some(holder) => format!(
                    "; {} holds {} without listening, as the Perch app does when it runs on this machine",
                    holder.map_or("another process".to_string(), |pid| format!("process {pid}")),
                    perch_engine::lock::path(&home).display()
                ),
                None => String::new(),
            };
            return Err(format!(
                "no engine answered on {} within {} s{holder}; the engine's log is in {}",
                socket.display(),
                STARTING.as_secs(),
                perch_engine::log_file::dir().display(),
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// `perchd serve`, detached: a session of its own, so the ssh session
/// ending does not end it, and no stdio of the bridge's, which belongs to
/// the wire. It logs to its file.
fn start() -> Result<Child, String> {
    let exe = std::env::current_exe().map_err(|e| format!("could not find perchd itself: {e}"))?;
    let mut command = Command::new(&exe);
    command
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for name in crate::serve::SESSION_ONLY {
        command.env_remove(name);
    }
    // SAFETY: setsid is async-signal-safe, and nothing else runs between
    // the fork and the exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .map_err(|e| format!("could not start {} serve: {e}", exe.display()))
}
