//! `perchd serve`: the engine as a service. It lives until SIGTERM or
//! SIGINT, whoever connects and leaves, and stops then as quitting the app
//! does: runs stopped and recorded Interrupted, children reaped, the home
//! let go and the socket removed.

use std::process::ExitCode;

use perch_engine::backend::{Engine, Lifetime};
use perch_engine::log_file;
use tokio::net::UnixListener;

use crate::socket;

pub fn run() -> ExitCode {
    prepare_environment();
    init_logging();
    tracing::info!(version = perch_engine::wire::VERSION, "perchd starting");
    match serve() {
        Ok(()) => {
            tracing::info!("perchd stopped");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!("perchd could not start: {error}");
            ExitCode::FAILURE
        }
    }
}

fn serve() -> Result<(), String> {
    // The home first: a second engine is turned away before it touches
    // the socket the first one listens on.
    let unstarted = Engine::build(Lifetime::Service).map_err(|e| e.to_string())?;
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
    let signals = {
        use tokio::signal::unix::{signal, SignalKind};
        let _in_runtime = runtime.enter();
        let term = signal(SignalKind::terminate());
        let int = signal(SignalKind::interrupt());
        match (term, int) {
            (Ok(term), Ok(int)) => (term, int),
            (Err(e), _) | (_, Err(e)) => {
                let _ = std::fs::remove_file(&socket);
                return Err(format!("could not take SIGTERM and SIGINT: {e}"));
            }
        }
    };
    // perchd keeps no settings of its own beyond the engine's.
    unstarted.start(|_| {});
    tracing::info!(socket = %socket.display(), "listening");
    runtime.spawn(accept(listener, engine.clone()));

    let (mut term, mut int) = signals;
    let signal = runtime.block_on(async {
        tokio::select! {
            _ = term.recv() => "SIGTERM",
            _ = int.recv() => "SIGINT",
        }
    });
    tracing::info!(signal, "stopping");
    // Gone first, and not after: a bridge that comes while this engine
    // stops starts the next one, which takes the home once this one has
    // let it go and listens on a socket of its own at the same path.
    let _ = std::fs::remove_file(&socket);
    engine.shutdown();
    engine.wait_stopped();
    Ok(())
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
    // A systemd unit, like a Finder launch, gets a bare PATH.
    if let Some(path) = perch_agent::path_with_install_dirs(
        std::env::var_os("PATH").as_deref(),
        &perch_agent::install_dirs(),
    ) {
        std::env::set_var("PATH", path);
    }
}

/// To stderr, which a service manager keeps, and to the log file a day in
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
    let stderr = tracing_subscriber::fmt::layer()
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
