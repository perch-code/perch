//! `perchd serve`: the engine as a service. It lives until SIGTERM, SIGINT
//! or SIGHUP, or a client asks it to stop over the wire (`perchd bridge
//! --replace`), whoever connects and leaves, and stops then as quitting
//! the app does: runs stopped and recorded Interrupted, children reaped,
//! the home let go and the socket removed. A second SIGTERM or SIGINT
//! while it stops ends it at once.
//!
//! With `--exit-after-idle <seconds>` it also stops, the same way, once
//! it has had no client connected and no run queued or under way for
//! that long. A bridge starts its engine so: the app brings its engine
//! with it and starts it when it connects, so one nobody is using has no
//! reason to stay. A run keeps it, waiting on an answer included, so a
//! Mac closed over its work finds the work done. One run by hand, with
//! no flag, stays until it is stopped.
//!
//! An engine that cannot start says why in `$PERCH_HOME/run/serve-failed`
//! as well as in its log, for the bridge that started it to pass on; one
//! that starts removes the file.

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// What `perchd serve`'s arguments ask: `Some(None)` for none, to run
/// until stopped; `Some(Some(idle))` for `--exit-after-idle <seconds>`;
/// None for ones it does not take.
pub fn parse(args: &[String]) -> Option<Option<Duration>> {
    match args {
        [] => Some(None),
        [flag, seconds] if flag == "--exit-after-idle" => seconds
            .parse::<u64>()
            .ok()
            .filter(|seconds| *seconds > 0)
            .map(|seconds| Some(Duration::from_secs(seconds))),
        _ => None,
    }
}

/// How often an engine that exits when idle looks at whether it is.
const IDLE_LOOK: Duration = Duration::from_secs(15);

/// How long an engine has been idle at `now`, keeping in `since` when it
/// was last seen in use: `used` when it has a client or a run, or had a
/// client since the last look. In use, it has not been.
fn idle_for(since: &mut Instant, now: Instant, used: bool) -> Duration {
    if used {
        *since = now;
    }
    now.saturating_duration_since(*since)
}

/// The streams the socket has taken: how many are open, and how many
/// there have ever been. A stream counts from when it is taken, before
/// its client has said hello, so a bridge that has just connected is seen
/// while its app's first words are still on their way over ssh.
#[derive(Default)]
struct Taken {
    open: AtomicUsize,
    ever: AtomicU64,
}

/// Done once the engine has had no client, no run and no other work for
/// `idle`; never, with none given. It is looked at every [`IDLE_LOOK`],
/// or more often for a shorter `idle`, so the engine may stay that much
/// longer. A client that came and went between two looks counts.
async fn gone_idle(engine: Engine, taken: Arc<Taken>, idle: Option<Duration>) {
    let Some(idle) = idle else {
        return std::future::pending().await;
    };
    let mut since = Instant::now();
    let mut seen = taken.ever.load(Ordering::SeqCst);
    let look = IDLE_LOOK.min(idle / 4).max(Duration::from_millis(50));
    loop {
        tokio::time::sleep(look).await;
        let ever = taken.ever.load(Ordering::SeqCst);
        let used = taken.open.load(Ordering::SeqCst) > 0
            || engine.connections() > 0
            || engine.active_runs() > 0
            || engine.other_work() > 0
            || ever != seen;
        seen = ever;
        if idle_for(&mut since, Instant::now(), used) >= idle {
            return;
        }
    }
}

pub fn run(idle: Option<Duration>) -> ExitCode {
    prepare_environment();
    init_logging();
    // Read while the binary is still there under its own name.
    let build = perch_engine::wire::build();
    tracing::info!(
        version = perch_engine::wire::VERSION,
        build,
        exits_after_idle_secs = idle.map(|idle| idle.as_secs()),
        "perchd starting"
    );
    match serve(idle) {
        Ok(()) => {
            tracing::info!("perchd stopped");
            ExitCode::SUCCESS
        }
        Err(Failed::Held(why)) => {
            tracing::error!("perchd could not start: {why}");
            say_failed(&why);
            ExitCode::from(HOME_HELD)
        }
        Err(Failed::Other(why)) => {
            tracing::error!("perchd could not start: {why}");
            say_failed(&why);
            ExitCode::FAILURE
        }
    }
}

/// Refuse a database a newer Perch has moved on, `found` being its schema.
/// The app's engine runs on without its database and says so in a window;
/// a host's has nobody to tell, and would run with its runs unrecorded, so
/// it does not start. The bridge that started it passes the reason on.
fn newer_database(path: &std::path::Path, found: Option<&str>) -> Result<(), String> {
    let current = perch_github::cache::SCHEMA_VERSION;
    match found.and_then(|found| found.parse::<u32>().ok()) {
        Some(found) if found > current => Err(format!(
            "{} was moved on by a newer Perch (schema {found}), and perchd {} reads up to schema {current}. Update Perch on the Mac, or start the newer perchd",
            path.display(),
            perch_engine::wire::VERSION,
        )),
        _ => Ok(()),
    }
}

/// Point `$PERCH_HOME/bin/perchd` at this build, for an agent on the
/// host: `~/.perch/bin/perchd context` is then the build of the engine
/// that has the database, which another build may not read. A failure
/// is a line in the log; the engine runs without it.
fn link_for_agents() {
    let Ok(exe) = crate::own_exe() else {
        return;
    };
    let bin = perch_github::config::perch_home().join("bin");
    if let Err(error) = link_in(&bin, &exe) {
        tracing::warn!(%error, bin = %bin.display(), "could not link perchd for agents");
    }
}

/// `bin/perchd` as a link to `exe`, replaced whole. Something there that
/// is not a link is someone's own, and stays.
fn link_in(bin: &std::path::Path, exe: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(bin)?;
    let link = bin.join("perchd");
    match std::fs::symlink_metadata(&link) {
        Ok(found) if !found.file_type().is_symlink() => return Ok(()),
        Ok(_) if std::fs::read_link(&link).is_ok_and(|to| to == exe) => return Ok(()),
        _ => {}
    }
    // A dot name the bridge's sweep leaves alone for an hour, which is
    // ample for a rename.
    let temp = bin.join(format!(".perchd-link-{}", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    std::os::unix::fs::symlink(exe, &temp)?;
    std::fs::rename(&temp, &link).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// Where an engine that could not start says why.
pub fn failed_path() -> std::path::PathBuf {
    socket::path().with_file_name("serve-failed")
}

/// Leave `why` for the bridge that started this engine, owner-only. The
/// directory may not be there yet: the home is taken before the socket's
/// directory is made.
fn say_failed(why: &str) {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let path = failed_path();
    let written = (|| {
        let dir = path.parent().unwrap_or(std::path::Path::new("."));
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        writeln!(file, "{}", one_line(why))
    })();
    if let Err(error) = written {
        tracing::warn!(%error, path = %path.display(), "could not say why perchd failed");
    }
}

/// `why` on one line, as the bridge's stderr has the app show its last.
pub fn one_line(why: &str) -> String {
    why.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// On a Mac, hold off idle sleep for as long as the engine runs: a host
/// is there to keep working with nobody at it, and a Mac left alone
/// sleeps, runs and all. `caffeinate` holds the assertion and watches this
/// process, so it lets go when the engine exits however it does. A lid
/// shut still sleeps the machine. Linux hosts sleep only when told to.
fn stay_awake() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let held = std::process::Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match held {
        Ok(_) => tracing::info!("holding off idle sleep while the engine runs"),
        Err(error) => tracing::warn!(%error, "could not hold off idle sleep"),
    }
}

fn serve(idle: Option<Duration>) -> Result<(), Failed> {
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
    if let Some(path) = perch_github::config::db_path() {
        let found = perch_github::Cache::peek_meta(&path, "schema_version");
        newer_database(&path, found.as_deref()).map_err(Failed::Other)?;
    }
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
    link_for_agents();
    // perchd keeps no settings of its own beyond the engine's.
    unstarted.start(|_| {});
    tracing::info!(socket = %socket.display(), "listening");
    stay_awake();
    let _ = std::fs::remove_file(failed_path());
    let taken = Arc::new(Taken::default());
    runtime.spawn(accept(listener, engine.clone(), taken.clone()));

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
    // A client asking it to stop, another build's bridge making way for
    // its own engine, stops it as a signal does; and so does having had
    // nobody and nothing to run for as long as it was told to wait.
    let signal = runtime.block_on(async {
        tokio::select! {
            biased;
            _ = stopped_by_itself => None,
            signal = signals.next() => Some(signal.unwrap_or("no signal; the runtime went")),
            () = engine.stop_asked() => Some("a stop asked for over the wire"),
            () = gone_idle(engine.clone(), taken, idle) => Some("no client and no run for long enough"),
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
async fn accept(listener: UnixListener, engine: Engine, taken: Arc<Taken>) {
    /// One stream, counted open until it is done with however it ends.
    struct Open(Arc<Taken>);
    impl Drop for Open {
        fn drop(&mut self) {
            self.0.open.fetch_sub(1, Ordering::SeqCst);
        }
    }
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                taken.open.fetch_add(1, Ordering::SeqCst);
                taken.ever.fetch_add(1, Ordering::SeqCst);
                let open = Open(taken.clone());
                let engine = engine.clone();
                tokio::spawn(async move {
                    let _open = open;
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

    #[test]
    fn the_link_for_agents_follows_the_build_and_leaves_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let (one, two) = (dir.path().join("perchd-1"), dir.path().join("perchd-2"));
        link_in(&bin, &one).unwrap();
        assert_eq!(std::fs::read_link(bin.join("perchd")).unwrap(), one);
        link_in(&bin, &two).unwrap();
        assert_eq!(std::fs::read_link(bin.join("perchd")).unwrap(), two);
        assert_eq!(
            std::fs::read_dir(&bin).unwrap().count(),
            1,
            "a temporary link stayed"
        );

        std::fs::remove_file(bin.join("perchd")).unwrap();
        std::fs::write(bin.join("perchd"), "mine").unwrap();
        link_in(&bin, &one).unwrap();
        assert_eq!(std::fs::read_to_string(bin.join("perchd")).unwrap(), "mine");
    }

    /// A forwarded agent comes with an ssh session's own variables; a
    /// host's own agent, from a desktop login, comes alone and is kept.
    #[test]
    fn a_database_from_a_newer_perch_is_refused() {
        let path = std::path::Path::new("/home/c/.perch/perch.db");
        let current = perch_github::cache::SCHEMA_VERSION;
        assert!(newer_database(path, None).is_ok());
        assert!(newer_database(path, Some(&current.to_string())).is_ok());
        assert!(newer_database(path, Some("3")).is_ok());
        let why = newer_database(path, Some(&(current + 1).to_string())).unwrap_err();
        assert!(why.contains("newer Perch"), "{why}");
        assert!(why.contains(&format!("schema {}", current + 1)), "{why}");
    }

    #[test]
    fn an_engine_is_idle_only_since_it_was_last_in_use() {
        let start = Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        let mut since = start;
        // In use, it has not been idle at all.
        assert_eq!(idle_for(&mut since, at(60), true), Duration::ZERO);
        assert_eq!(idle_for(&mut since, at(120), true), Duration::ZERO);
        // From when it was last seen in use, not from when it started.
        assert_eq!(
            idle_for(&mut since, at(300), false),
            Duration::from_secs(180)
        );
        assert_eq!(
            idle_for(&mut since, at(1020), false),
            Duration::from_secs(900)
        );
        // In use again, the wait starts over.
        assert_eq!(idle_for(&mut since, at(1030), true), Duration::ZERO);
        assert_eq!(
            idle_for(&mut since, at(1040), false),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn serve_takes_how_long_to_wait_idle_or_nothing() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(parse(&args(&[])), Some(None));
        assert_eq!(
            parse(&args(&["--exit-after-idle", "900"])),
            Some(Some(Duration::from_secs(900)))
        );
        assert_eq!(parse(&args(&["--exit-after-idle"])), None);
        assert_eq!(parse(&args(&["--exit-after-idle", "0"])), None);
        assert_eq!(parse(&args(&["--exit-after-idle", "soon"])), None);
        assert_eq!(parse(&args(&["--daemon"])), None);
    }

    #[test]
    fn why_it_failed_is_one_line() {
        assert_eq!(
            one_line("the cache was made by a newer Perch:\n  schema 9,\tthis is 8\n"),
            "the cache was made by a newer Perch: schema 9, this is 8"
        );
    }

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
