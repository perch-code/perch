//! `perchd serve`: the engine as a service. It lives until SIGTERM, SIGINT
//! or SIGHUP, whoever connects and leaves, and stops then as quitting the
//! app does: runs stopped and recorded Interrupted, children reaped, the
//! home let go and the socket removed. A second SIGTERM or SIGINT while
//! it stops ends it at once.

use std::io::IsTerminal;
use std::process::ExitCode;

use perch_engine::backend::{Engine, Lifetime};
use perch_engine::log_file;
use tokio::net::UnixListener;

use crate::socket;

/// The exit when another engine has the Perch home, so a script can tell
/// "one is already running" from a failure.
pub const HOME_HELD: u8 = 75;

/// What an ssh session sets that the engine, which outlives the session,
/// must not keep: an agent socket that may be forwarded from the client's
/// machine, which every run the engine starts would be handed, and the
/// session's own addresses and terminal. A bridge drops them all from the
/// engine it starts; `perchd serve` drops what [`from_a_session`] says.
pub const SESSION_ONLY: [&str; 4] = ["SSH_AUTH_SOCK", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"];

/// Why serve failed.
enum Failed {
    /// Another engine has the home.
    Held(String),
    Other(String),
}

impl From<String> for Failed {
    fn from(why: String) -> Self {
        Failed::Other(why)
    }
}

pub fn run() -> ExitCode {
    prepare_environment();
    init_logging();
    tracing::info!(version = perch_engine::wire::VERSION, "perchd starting");
    match serve() {
        Ok(()) => {
            tracing::info!("perchd stopped");
            ExitCode::SUCCESS
        }
        Err(Failed::Held(why)) => {
            tracing::error!("perchd could not start: {why}");
            ExitCode::from(HOME_HELD)
        }
        Err(Failed::Other(why)) => {
            tracing::error!("perchd could not start: {why}");
            ExitCode::FAILURE
        }
    }
}

fn serve() -> Result<(), Failed> {
    // The home first: a second engine is turned away before it touches
    // the socket the first one listens on.
    let unstarted = Engine::build(Lifetime::Service).map_err(|error| {
        let held = error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<perch_engine::lock::LockError>())
            .is_some_and(|lock| matches!(lock, perch_engine::lock::LockError::Held { .. }));
        if held {
            Failed::Held(error.to_string())
        } else {
            Failed::Other(error.to_string())
        }
    })?;
    let engine = unstarted.engine().clone();
    let runtime = engine.runtime().clone();
    let socket = socket::path();
    socket::prepare(&socket)?;
    let listener = socket::bind(&socket)?;
    let listener = {
        let _in_runtime = runtime.enter();
        listener
            .set_nonblocking(true)
            .and_then(|()| UnixListener::from_std(listener))
            .map_err(|e| format!("could not listen on {}: {e}", socket.display()))?
    };
    let mut signals = {
        let _in_runtime = runtime.enter();
        match Signals::take() {
            Ok(signals) => signals,
            Err(e) => {
                let _ = std::fs::remove_file(&socket);
                return Err(format!("could not take SIGTERM, SIGINT and SIGHUP: {e}").into());
            }
        }
    };
    // perchd keeps no settings of its own beyond the engine's.
    unstarted.start(|_| {});
    tracing::info!(socket = %socket.display(), "listening");
    runtime.spawn(accept(listener, engine.clone()));

    // An engine that stops by itself has failed: its thread panicked. The
    // exit says so, and the next bridge starts another.
    let (stopped, stopped_by_itself) = tokio::sync::oneshot::channel();
    std::thread::spawn({
        let engine = engine.clone();
        move || {
            engine.wait_stopped();
            let _ = stopped.send(());
        }
    });
    let signal = runtime.block_on(async {
        tokio::select! {
            biased;
            _ = stopped_by_itself => None,
            signal = signals.next() => Some(signal.unwrap_or("no signal; the runtime went")),
        }
    });
    let Some(signal) = signal else {
        let _ = std::fs::remove_file(&socket);
        return Err("the engine stopped by itself; the log says why"
            .to_string()
            .into());
    };
    tracing::info!(signal, "stopping");
    // Gone first, and not after: a bridge that comes while this engine
    // stops starts the next one, which takes the home once this one has
    // let it go and listens on a socket of its own at the same path.
    let _ = std::fs::remove_file(&socket);
    // Stopping waits for the command before it, which can be a long one:
    // a second signal is the reader saying not to wait. The kernel lets
    // the home go as the process ends. A hangup is not the reader: an ssh
    // session that drops sends two, the shell's and then the kernel's as
    // the shell exits.
    runtime.spawn(async move {
        // None as the runtime goes, the engine stopped: nothing to cut short.
        while let Some(again) = signals.next().await {
            if again == "SIGHUP" {
                tracing::info!("another SIGHUP while stopping; still stopping");
                continue;
            }
            tracing::warn!(signal = again, "stopping at once");
            std::process::exit(1);
        }
    });
    engine.shutdown();
    engine.wait_stopped();
    Ok(())
}

/// The signals that stop the engine. SIGHUP is one: an engine started by
/// hand in an ssh session that drops would otherwise die there with its
/// runs left recorded as running.
struct Signals {
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
    hup: tokio::signal::unix::Signal,
}

impl Signals {
    /// Called inside the runtime.
    fn take() -> std::io::Result<Signals> {
        use tokio::signal::unix::{signal, SignalKind};
        Ok(Signals {
            term: signal(SignalKind::terminate())?,
            int: signal(SignalKind::interrupt())?,
            hup: signal(SignalKind::hangup())?,
        })
    }

    /// The next one to come, by name. None once no more can: the runtime
    /// is going.
    async fn next(&mut self) -> Option<&'static str> {
        tokio::select! {
            Some(()) = self.term.recv() => Some("SIGTERM"),
            Some(()) = self.int.recv() => Some("SIGINT"),
            Some(()) = self.hup.recv() => Some("SIGHUP"),
            else => None,
        }
    }
}

/// Serve each client that connects, for as long as the engine runs.
async fn accept(listener: UnixListener, engine: Engine) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let engine = engine.clone();
                tokio::spawn(async move {
                    if let Err(error) = perch_engine::wire::serve(engine, stream).await {
                        tracing::info!(%error, "a client's connection ended");
                    }
                });
            }
            // Out of descriptors, most likely: the next try may do better.
            Err(error) => {
                tracing::warn!(%error, "could not accept a client");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// The environment the engine hands what it starts, as the app sets it up:
/// see perch-app's `main`. Done before any thread starts, since nothing may
/// read the environment while it changes.
fn prepare_environment() {
    for name in ["RUSTUP_TOOLCHAIN", "RUSTUP_TOOLCHAIN_SOURCE", "CARGO"] {
        std::env::remove_var(name);
    }
    for name in from_a_session(|name| std::env::var_os(name).is_some()) {
        std::env::remove_var(name);
    }
    // The agents' install dirs, as the app adds them, for a login shell
    // whose PATH leaves them out.
    if let Some(path) = perch_agent::path_with_install_dirs(
        std::env::var_os("PATH").as_deref(),
        &perch_agent::install_dirs(),
    ) {
        std::env::set_var("PATH", path);
    }
}

/// Which of [`SESSION_ONLY`] `perchd serve` drops, given which are set:
/// all of them in an ssh session, run by hand in a login or in a tmux
/// started from one. Outside one, run from a desktop login on the host,
/// an agent socket is the host's own (gnome-keyring, say), and git over
/// ssh with a key behind a passphrase needs it.
fn from_a_session(set: impl Fn(&str) -> bool) -> &'static [&'static str] {
    if ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .into_iter()
        .any(set)
    {
        &SESSION_ONLY
    } else {
        &[]
    }
}

/// To stderr, for a person running it by hand, and to the log file a day in
/// `$PERCH_HOME/logs`, which a detached engine's stderr is not. Never to
/// stdout, which a bridge's client reads as the wire.
fn init_logging() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;

    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "perch=info,warn".into())
    };
    // Colour only for a person: a file would keep the codes.
    let stderr = tracing_subscriber::fmt::layer()
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .with_filter(filter());
    let file = log_file::appender(&log_file::dir()).ok().map(|file| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(file)
            .with_filter(filter())
    });
    tracing_subscriber::registry()
        .with(stderr)
        .with(file)
        .init();
    log_file::install_panic_hook();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A forwarded agent comes with an ssh session's own variables; a
    /// host's own agent, from a desktop login, comes alone and is kept.
    #[test]
    fn only_an_ssh_session_s_agent_is_dropped() {
        let session = |name: &str| matches!(name, "SSH_AUTH_SOCK" | "SSH_CONNECTION" | "SSH_TTY");
        assert_eq!(from_a_session(session), SESSION_ONLY);
        let tmux = |name: &str| matches!(name, "SSH_AUTH_SOCK" | "SSH_CLIENT");
        assert_eq!(from_a_session(tmux), SESSION_ONLY);
        let desktop = |name: &str| name == "SSH_AUTH_SOCK";
        assert!(from_a_session(desktop).is_empty());
    }
}
