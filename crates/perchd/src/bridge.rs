//! `perchd bridge`: what `ssh <host> perchd bridge` runs. Its stdin and
//! stdout become a connection to the engine's socket, byte for byte, until
//! either side closes. Nothing else is ever written to stdout: the app
//! reads it as the wire. What goes wrong goes to stderr.
//!
//! With no engine answering, the bridge starts `perchd serve` in a session
//! of its own, so the engine outlives the ssh session that started it, and
//! waits for its socket.
//!
//! A bridge the app installed clears the older builds beside it as it
//! starts (see `cleanup`).
//!
//! `--replace` and `--replace-if-idle` make way for this bridge's own
//! build: an engine of another build is asked over the wire to stop (it
//! stops as SIGTERM stops it), and once it has let the home go, this
//! bridge starts its own. Nothing is signalled or killed by pid. An engine
//! of this build is joined as it is. With `--replace-if-idle`, an engine
//! with runs or clients is left alone, and the bridge says
//! `perch-busy: <version> <runs> <clients>` and exits 98.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime};

use perch_engine::lock::EngineLock;
use perch_engine::wire::provision::{Replace, BUSY_EXIT, BUSY_SAID};
use perch_engine::wire::{self, Answer};

use crate::socket;

/// How long an engine started here gets to answer.
const STARTING: Duration = Duration::from_secs(10);

/// How often the socket is tried while it starts.
const POLL: Duration = Duration::from_millis(25);

/// How long an engine asked to stop gets to let the home go: its runs
/// are stopped and recorded first.
const STOPPING: Duration = Duration::from_secs(60);

/// How long an engine gets to answer a stop.
const ANSWER: Duration = Duration::from_secs(10);

/// How the bridge's arguments say to treat an engine of another build,
/// or None for ones it does not take.
pub fn parse(args: &[String]) -> Option<Replace> {
    [Replace::No, Replace::IfIdle, Replace::Always]
        .into_iter()
        .find(|replace| {
            let flag: Vec<&str> = replace.flag().into_iter().collect();
            args.iter().map(String::as_str).eq(flag)
        })
}

/// How a bridge that could not join ends.
enum Ended {
    /// Why, for stderr.
    Failed(String),
    /// The engine is busy: what `--replace-if-idle` says instead.
    Busy(wire::About),
}

pub fn run(replace: Replace) -> ExitCode {
    crate::cleanup::start();
    let since = SystemTime::now();
    let socket = socket::path();
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) if replace == Replace::No => Ok(stream),
        Ok(stream) => make_way(stream, &socket, replace, since),
        Err(_) => start_and_connect(&socket, since).map_err(Ended::Failed),
    };
    match stream {
        Ok(stream) => join(stream),
        Err(Ended::Failed(why)) => {
            eprintln!("perchd bridge: {}", crate::serve::one_line(&why));
            ExitCode::FAILURE
        }
        Err(Ended::Busy(about)) => {
            eprintln!("{}", busy_line(&about));
            ExitCode::from(BUSY_EXIT)
        }
    }
}

/// What a busy engine has the bridge say, the app's cue to offer a
/// restart rather than make one.
pub fn busy_line(about: &wire::About) -> String {
    format!(
        "{BUSY_SAID} {} {} {}",
        about.engine_version, about.runs, about.clients
    )
}

/// Ask the engine on `stream` to stop unless it is this bridge's build,
/// wait for it to let the home go, and start this build's: a stream to
/// the engine to join, either way.
fn make_way(
    stream: UnixStream,
    socket: &Path,
    replace: Replace,
    since: SystemTime,
) -> Result<UnixStream, Ended> {
    let own = wire::build().to_string();
    let asked = Socket::of(socket);
    let answer = ask_to_stop(stream, replace == Replace::IfIdle, own.clone())
        .map_err(|why| Ended::Failed(format!("could not ask the engine to stop: {why}")))?;
    match answer {
        Answer::Staying(_) => match UnixStream::connect(socket) {
            Ok(stream) => Ok(stream),
            // Gone since it answered: started again, as with none.
            Err(_) => start_and_connect(socket, since).map_err(Ended::Failed),
        },
        Answer::Busy(about) => Err(Ended::Busy(about)),
        Answer::Stopping(about) => {
            let home = perch_github::config::perch_home();
            let held = || EngineLock::holder(&home).is_some();
            match wait_let_go(socket, asked, STOPPING, held) {
                Ok(Some(next)) => Ok(next),
                Ok(None) => start_and_connect(socket, since).map_err(Ended::Failed),
                Err(()) => Err(Ended::Failed(format!(
                    "perchd {} said it would stop to make way for {own}, and was still running after {} s; its log is in {}",
                    about.build,
                    STOPPING.as_secs(),
                    perch_engine::log_file::dir().display(),
                ))),
            }
        }
        Answer::Refused { reason, about } if about.protocol == 0 => Err(Ended::Failed(format!(
            "the engine running here is from before Perch could replace one, and would not stop ({reason}); stop its perchd serve on this host and connect again"
        ))),
        Answer::Refused { reason, .. } => Err(Ended::Failed(format!(
            "the engine running here would not stop: {reason}"
        ))),
        Answer::Welcome(_) => Err(Ended::Failed(
            "the engine let the bridge in when asked to stop".to_string(),
        )),
    }
}

/// The engine's answer to a stop, sent on `stream`.
fn ask_to_stop(stream: UnixStream, if_idle: bool, own: String) -> Result<Answer, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        stream.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stream = tokio::net::UnixStream::from_std(stream).map_err(|e| e.to_string())?;
        tokio::time::timeout(ANSWER, wire::ask_to_stop(stream, if_idle, Some(own)))
            .await
            .map_err(|_| format!("it said nothing within {} s", ANSWER.as_secs()))?
            .map_err(|e| e.to_string())
    })
}

/// Which socket file an engine listens on. A stopping engine removes its
/// socket at once, and the next one makes a new file at the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Socket {
    inode: u64,
    changed: (i64, i64),
}

impl Socket {
    fn of(path: &Path) -> Option<Socket> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some(Socket {
            inode: meta.ino(),
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }
}

/// Wait up to `within` for an engine stopping, which listened on `asked`,
/// to let the home go: its socket no longer answering, and `held` false.
/// Or for another engine to answer in its place, the one another bridge
/// started once the home was free, which this one joins rather than
/// waiting out the minute: `Some` of a stream to it.
fn wait_let_go(
    socket: &Path,
    asked: Option<Socket>,
    within: Duration,
    held: impl Fn() -> bool,
) -> Result<Option<UnixStream>, ()> {
    let deadline = Instant::now() + within;
    let mut gone = false;
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) if gone || Socket::of(socket).is_some_and(|now| Some(now) != asked) => {
                return Ok(Some(stream));
            }
            Ok(_) => {}
            Err(_) if !held() => return Ok(None),
            Err(_) => gone = true,
        }
        if Instant::now() >= deadline {
            return Err(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
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

/// What an engine started since `since` left to say why it could not
/// start, on one line. A second is allowed for a file system that keeps
/// times to the second.
fn said_failed(since: SystemTime) -> Option<String> {
    let path = crate::serve::failed_path();
    let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
    let since = since.checked_sub(Duration::from_secs(1)).unwrap_or(since);
    if modified < since {
        return None;
    }
    let said = crate::serve::one_line(&std::fs::read_to_string(&path).ok()?);
    (!said.is_empty()).then_some(said)
}

/// Start an engine and connect to it once it answers. One that exits
/// without answering, turned away from the home, most likely by an engine
/// still stopping or racing another bridge's, is tried again until the
/// time is up. One that failed for another reason, which it left in
/// `serve-failed`, a cache from a newer Perch say, is not: the bridge
/// says that reason.
fn start_and_connect(socket: &Path, since: SystemTime) -> Result<UnixStream, String> {
    let deadline = Instant::now() + STARTING;
    let mut child: Option<Child> = None;
    let mut last_start = None;
    loop {
        if let Ok(stream) = UnixStream::connect(socket) {
            return Ok(stream);
        }
        let running = match child.as_mut().map(Child::try_wait) {
            Some(Ok(None)) => true,
            Some(Ok(Some(status))) => {
                let held = status.code() == Some(i32::from(crate::serve::HOME_HELD));
                if !held && !status.success() {
                    if let Some(why) = said_failed(since) {
                        return Err(format!(
                            "the engine did not start: {why}. Its log is in {}",
                            perch_engine::log_file::dir().display()
                        ));
                    }
                }
                false
            }
            Some(Err(_)) | None => false,
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
                    "; {} holds {} without listening: an engine still stopping, or the Perch app running on this machine, which Perch can't replace (quit it there)",
                    holder.map_or("another process".to_string(), |pid| format!("process {pid}")),
                    perch_engine::lock::path(&home).display()
                ),
                None => match said_failed(since) {
                    Some(why) => format!("; the last one to try said: {why}"),
                    None => String::new(),
                },
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn a_replacing_bridge_joins_an_engine_started_in_the_old_one_s_place() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("engine.sock");
        let quick = Duration::from_millis(400);

        // The old engine still listening: the wait runs out.
        let old = UnixListener::bind(&socket).unwrap();
        let asked = Socket::of(&socket);
        assert!(asked.is_some());
        assert!(wait_let_go(&socket, asked, quick, || true).is_err());

        // Gone, and the home let go: this bridge starts its own.
        drop(old);
        std::fs::remove_file(&socket).unwrap();
        assert!(matches!(
            wait_let_go(&socket, asked, quick, || false),
            Ok(None)
        ));

        // Gone, and another bridge's engine already listening at the same
        // path, holding the home: joined, though the gap was never seen.
        let next = UnixListener::bind(&socket).unwrap();
        assert_ne!(Socket::of(&socket), asked);
        let joined = wait_let_go(&socket, asked, quick, || true).unwrap();
        assert!(joined.is_some());
        drop(next);
    }

    #[test]
    fn a_replacing_bridge_that_saw_the_socket_go_joins_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("engine.sock");
        let old = UnixListener::bind(&socket).unwrap();
        let asked = Socket::of(&socket);
        drop(old);
        std::fs::remove_file(&socket).unwrap();
        let path = socket.clone();
        let next = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            UnixListener::bind(&path).unwrap()
        });
        // The home stays held throughout, by the old engine and then the
        // next: only the socket coming back ends the wait.
        let joined = wait_let_go(&socket, asked, Duration::from_secs(5), || true).unwrap();
        assert!(joined.is_some());
        drop(next.join().unwrap());
    }

    #[test]
    fn the_flags_the_app_passes_are_the_ones_taken() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(parse(&args(&[])), Some(Replace::No));
        assert_eq!(parse(&args(&["--replace"])), Some(Replace::Always));
        assert_eq!(parse(&args(&["--replace-if-idle"])), Some(Replace::IfIdle));
        assert_eq!(parse(&args(&["--replace", "--replace"])), None);
        assert_eq!(parse(&args(&["--force"])), None);
        let about = wire::About {
            engine_version: "0.9.0".into(),
            runs: 2,
            clients: 1,
            ..wire::About::default()
        };
        assert_eq!(busy_line(&about), "perch-busy: 0.9.0 2 1");
        assert_eq!(
            perch_engine::wire::provision::busy_line(&busy_line(&about)).map(|b| b.runs),
            Some(2)
        );
    }
}
