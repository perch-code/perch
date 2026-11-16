//! perchd as a host runs it: the built binary, on a Perch home of its own,
//! reached through `perchd bridge` as ssh would reach it.
//!
//! The engine here can find no GitHub token and no agent: the environment
//! has neither token variable, `gh` on PATH is a stand-in that knows no
//! token, and config.toml points every agent at a path that is not there.
//! So nothing it does goes to the network, or reads the reader's own home.
//! Its one repository is a local one made in the test's directory, with
//! no remote.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use perch_engine::backend::{Backend, Event, WindowId, NO_WINDOW};
use perch_engine::lock::EngineLock;
use perch_engine::wire;
use perch_github::Item;

const PERCHD: &str = env!("CARGO_BIN_EXE_perchd");

/// Long enough for a debug engine on a busy machine; what is waited for
/// usually comes in well under a second.
const PATIENCE: Duration = Duration::from_secs(30);

/// A Perch home in a directory of its own, and the environment perchd
/// runs in there.
struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        // Under /tmp: a socket's path has to fit in about a hundred bytes,
        // and the system's temporary directory can be deep.
        let dir = tempfile::Builder::new()
            .prefix("perchd")
            .tempdir_in("/tmp")
            .unwrap();
        let home = Home { dir };
        for sub in ["perch", "home", "bin"] {
            std::fs::create_dir_all(home.dir.path().join(sub)).unwrap();
        }
        // Found ahead of any real gh, and knowing no token.
        let gh = home.dir.path().join("bin/gh");
        std::fs::write(&gh, "#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            home.perch().join("config.toml"),
            "[agent]\nbinary = \"/nonexistent/claude\"\n\n\
             [agent.codex]\nbinary = \"/nonexistent/codex\"\n\n\
             [agent.gemini]\nbinary = \"/nonexistent/agy\"\n",
        )
        .unwrap();
        home
    }

    /// `$PERCH_HOME`.
    fn perch(&self) -> PathBuf {
        self.dir.path().join("perch")
    }

    fn socket(&self) -> PathBuf {
        self.perch().join("run/perchd.sock")
    }

    fn perchd(&self, args: &[&str]) -> Command {
        let mut command = Command::new(PERCHD);
        command
            .args(args)
            .env_clear()
            .env("PERCH_HOME", self.perch())
            .env("HOME", self.dir.path().join("home"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.dir.path().join("bin").display()),
            )
            .env("RUST_LOG", "perch=info,warn");
        command
    }

    fn perchd_async(&self, args: &[&str]) -> tokio::process::Command {
        tokio::process::Command::from(self.perchd(args))
    }

    /// The engine's pid, from its lock, while one holds it. Looked at
    /// without taking the lock, which would write this process's pid in
    /// it for a moment.
    fn engine_pid(&self) -> Option<i32> {
        EngineLock::holder(&self.perch())
            .flatten()
            .and_then(|pid| i32::try_from(pid).ok())
    }

    /// A local repository, `notes`, with a worktree on the branch
    /// `feature` that the engine lists as an item, declared in config.toml.
    fn with_repository(&self) {
        let code = self.dir.path().join("code");
        let root = code.join("notes");
        std::fs::create_dir_all(&root).unwrap();
        self.git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("README.md"), "notes\n").unwrap();
        self.git(&root, &["add", "README.md"]);
        self.git(&root, &["commit", "-q", "-m", "Start"]);
        let feature = code.join("notes-feature");
        self.git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                &feature.to_string_lossy(),
            ],
        );
        std::fs::write(feature.join("README.md"), "notes, more\n").unwrap();
        self.git(&feature, &["commit", "-q", "-am", "More"]);
        let config = self.perch().join("config.toml");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str(&format!(
            "\n[[repo]]\nroot = {:?}\n",
            root.to_string_lossy()
        ));
        std::fs::write(config, text).unwrap();
    }

    /// git in `dir`, as nobody in particular, reading no config of the
    /// reader's.
    fn git(&self, dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.name=Perch Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env_clear()
            .env("HOME", self.dir.path().join("home"))
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `perchd status`, its exit code and what it printed.
    fn status(&self) -> (Option<i32>, String) {
        let out = self.perchd(&["status"]).output().unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }
}

/// An engine a test started, detached by a bridge or not, is stopped
/// whatever the test did.
impl Drop for Home {
    fn drop(&mut self) {
        if let Some(pid) = self.engine_pid() {
            // SAFETY: a signal to a pid; no memory is involved.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            let deadline = Instant::now() + PATIENCE;
            while self.engine_pid().is_some() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Wait for `ready`, trying every little while.
fn eventually(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !ready() {
        assert!(Instant::now() < deadline, "waited too long for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A client through a bridge: the engine's handle and its events.
struct Client {
    bridge: tokio::process::Child,
    backend: Backend,
    events: Receiver<(WindowId, Event)>,
}

async fn client(home: &Home) -> Client {
    let mut bridge = home
        .perchd_async(&["bridge"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stream = tokio::io::join(bridge.stdout.take().unwrap(), bridge.stdin.take().unwrap());
    let (backend, events, welcome) = tokio::time::timeout(PATIENCE, wire::connect(stream))
        .await
        .expect("no welcome")
        .expect("the handshake failed");
    assert_eq!(welcome.protocol, wire::PROTOCOL);
    assert_eq!(welcome.engine_version, wire::VERSION);
    Client {
        bridge,
        backend,
        events,
    }
}

/// Wait for the first event `pick` takes, skipping the rest.
async fn wait<T>(
    events: &Receiver<(WindowId, Event)>,
    mut pick: impl FnMut(WindowId, &Event) -> Option<T>,
) -> T {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        while let Ok((window, event)) = events.try_recv() {
            if let Some(found) = pick(window, &event) {
                return found;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the event never came"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// What a resync, and only a resync, sends a client once it is running.
fn resynced(window: WindowId, event: &Event) -> Option<()> {
    (window == NO_WINDOW && matches!(event, Event::WorkDefaults(_))).then_some(())
}

/// Whether `client` heard a resync before the answer to a rename it asks
/// for now, from `window`, which only it hears.
async fn settle(client: &Client, window: &Backend) -> bool {
    window.rename_item("missing", None);
    let mut heard = false;
    wait(&client.events, |from, event| {
        heard |= resynced(from, event).is_some();
        matches!(event, Event::Notice(text) if text == "That item is gone" && from == window.window())
            .then_some(())
    })
    .await;
    heard
}

/// The item for the branch `feature`, in a list of items.
fn feature(event: &Event) -> Option<Item> {
    let Event::Items(items) = event else {
        return None;
    };
    items
        .iter()
        .find(|item| {
            item.checkout
                .as_ref()
                .is_some_and(|checkout| checkout.branch == "feature")
        })
        .cloned()
}

fn wait_for_socket(home: &Home) {
    eventually("the socket", || {
        std::os::unix::net::UnixStream::connect(home.socket()).is_ok()
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_bridged_clients_share_a_served_engine_that_stops_cleanly() {
    let home = Home::new();
    home.with_repository();
    let log = std::fs::File::create(home.dir.path().join("serve.log")).unwrap();
    let mut serve = home
        .perchd_async(&["serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    wait_for_socket(&home);
    let mode = |path: &Path| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode(&home.socket()), 0o600);
    assert_eq!(mode(home.socket().parent().unwrap()), 0o700);

    let a = client(&home).await;
    let b = client(&home).await;

    // Each client's resync is its own. A client that connected before the
    // engine had started heard its startup too, so what each heard is read
    // up to a reply only it gets, which comes after its resync's.
    let a_window = a.backend.open_window();
    a.backend.resync();
    assert!(settle(&a, &a_window).await, "a heard no resync");
    let b_window = b.backend.open_window();
    b.backend.resync();
    assert!(settle(&b, &b_window).await, "b heard no resync");
    assert!(!settle(&a, &a_window).await, "b's resync reached a");

    let (code, said) = home.status();
    assert_eq!(code, Some(0), "{said}");
    assert!(said.contains("clients: 2"), "{said}");
    assert!(said.contains("runs: 0"), "{said}");

    // One client opens the repository's worktree.
    a.backend.resync();
    let item = wait(&a.events, |_, event| feature(event)).await;
    let navigation = a_window.open_item(&item.id);
    let opened = wait(&a.events, |window, event| match event {
        Event::ReviewReady {
            navigation_id,
            item,
            ..
        } if window == a_window.window() && *navigation_id == navigation => Some(item.clone()),
        Event::ReviewFailed {
            navigation_id,
            message,
        } if *navigation_id == navigation => panic!("the open failed: {message}"),
        _ => None,
    })
    .await;
    assert_eq!(opened.id, item.id);

    // And goes, as an ssh session would, with the item open.
    let Client {
        mut bridge,
        backend,
        events,
    } = a;
    bridge.kill().await.unwrap();
    drop((backend, a_window, events));
    eventually("the engine to see a leave", || {
        home.status().1.contains("clients: 1")
    });
    // The other goes on hearing the engine, and acting on the item.
    b.backend.resync();
    assert!(settle(&b, &b_window).await, "b heard no resync");
    b_window.rename_item(&item.id, Some("Renamed by b".into()));
    wait(&b.events, |_, event| {
        feature(event).filter(|item| item.name.as_deref() == Some("Renamed by b"))
    })
    .await;

    // A third client, connecting now, hears the items as they stand.
    let c = client(&home).await;
    c.backend.resync();
    let listed = wait(&c.events, |window, event| {
        (window == NO_WINDOW).then(|| feature(event)).flatten()
    })
    .await;
    assert_eq!(listed.id, item.id);
    assert_eq!(listed.name.as_deref(), Some("Renamed by b"));
    let Client {
        mut bridge,
        backend,
        events,
    } = c;
    bridge.kill().await.unwrap();
    drop((backend, events));
    eventually("the engine to see c leave", || {
        home.status().1.contains("clients: 1")
    });

    // A second engine on the home is turned away, saying why.
    let second = tokio::time::timeout(PATIENCE, home.perchd_async(&["serve"]).output())
        .await
        .expect("a second serve kept running")
        .unwrap();
    // With its own exit, which the systemd unit does not restart on.
    assert_eq!(second.status.code(), Some(75));
    let said = String::from_utf8_lossy(&second.stderr);
    assert!(!said.contains('\x1b'), "colour off a terminal: {said:?}");
    assert!(said.contains("engine.lock"), "{said}");
    assert!(
        said.contains(&format!("process {}", serve.id().unwrap())),
        "{said}"
    );
    assert!(second.stdout.is_empty());
    // And the first never noticed.
    assert!(!settle(&b, &b_window).await);

    // SIGTERM stops it as quitting the app does, and takes the socket.
    // SAFETY: a signal to the child's pid.
    unsafe { libc::kill(serve.id().unwrap() as i32, libc::SIGTERM) };
    let ended = tokio::time::timeout(PATIENCE, serve.wait())
        .await
        .expect("serve kept running after SIGTERM")
        .unwrap();
    let log = std::fs::read_to_string(home.dir.path().join("serve.log")).unwrap();
    assert!(ended.success(), "{ended:?}\n{log}");
    assert!(log.contains("SIGTERM"), "{log}");
    assert!(!log.contains('\x1b'), "colour in a file: {log:?}");
    assert!(!home.socket().exists());
    EngineLock::take(&home.perch()).expect("the home was let go");
    // The bridge left hears the engine hang up, and ends.
    let mut b_bridge = b.bridge;
    let ended = tokio::time::timeout(PATIENCE, b_bridge.wait())
        .await
        .expect("the bridge outlived its engine")
        .unwrap();
    assert!(ended.success(), "{ended:?}");

    let (code, said) = home.status();
    assert_eq!(code, Some(3), "{said}");
    assert!(said.starts_with("not running"), "{said}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bridge_with_no_engine_starts_one_that_outlives_it() {
    let home = Home::new();
    assert_eq!(home.engine_pid(), None);
    let a = client(&home).await;
    a.backend.resync();
    wait(&a.events, resynced).await;
    let pid = home.engine_pid().expect("an engine holds the home");

    // The bridge goes and the engine stays, detached, for the next.
    let Client {
        mut bridge,
        backend,
        events,
    } = a;
    drop((backend, events));
    let ended = tokio::time::timeout(PATIENCE, bridge.wait())
        .await
        .expect("the bridge kept running with its client gone")
        .unwrap();
    assert!(ended.success(), "{ended:?}");
    let b = client(&home).await;
    b.backend.resync();
    wait(&b.events, resynced).await;
    assert_eq!(home.engine_pid(), Some(pid));
    let (_, said) = home.status();
    assert!(said.contains("clients: 1"), "{said}");

    // A hangup stops it as SIGTERM does: an engine run by hand in an ssh
    // session that drops is not left with its runs recorded as running.
    // SAFETY: a signal to the engine's pid, read from the lock it holds.
    unsafe { libc::kill(pid, libc::SIGHUP) };
    // The session's shell sends one and the kernel another as the shell
    // exits: the second does not cut the stop short.
    eventually("the engine to start stopping", || !home.socket().exists());
    if home.engine_pid() == Some(pid) {
        // SAFETY: as above, the engine still holding its home.
        unsafe { libc::kill(pid, libc::SIGHUP) };
    }
    eventually("the engine to stop", || home.engine_pid().is_none());
    assert!(!home.socket().exists());
    let logs = std::fs::read_dir(home.perch().join("logs")).unwrap();
    let log: String = logs
        .flatten()
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect();
    assert!(log.contains("SIGHUP"), "{log}");
    assert!(!log.contains("stopping at once"), "{log}");
}

#[test]
fn version_and_usage() {
    let home = Home::new();
    let out = home.perchd(&["--version"]).output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("perchd {} (protocol {})", wire::VERSION, wire::PROTOCOL)
    );
    let out = home.perchd(&["nonsense"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: perchd"));
}
