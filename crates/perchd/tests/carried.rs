//! An app's own engine, end to end, as the app gets it to a host: the
//! host says it has none, the app sends the one it carries in itself for
//! what the host is, the host keeps it under its hash, and its bridge
//! starts the engine. After that nothing is sent, and an engine of another build
//! with nothing running makes way.
//!
//! ssh is a stand-in that runs what it is given here, as the host's login
//! shell would, in a home of the test's own. As in `serve.rs`, the engine
//! there can find no GitHub token and no agent, so nothing it does goes
//! to the network or reads the reader's own home.
//!
//! One test, in a file of its own: `PERCH_SSH` is the whole process's.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use perch_engine::lock::EngineLock;
use perch_engine::wire::provision::{self, Carried, Platform, Source};
use perch_engine::wire::remote::{connect_once, Bridge, Connected};

const PERCHD: &str = env!("CARGO_BIN_EXE_perchd");

/// Long enough for a debug engine, and a debug binary sent, on a busy
/// machine.
const PATIENCE: Duration = Duration::from_secs(60);

/// The host: a directory with its home, its Perch home and its `PATH`.
struct Host {
    dir: tempfile::TempDir,
}

impl Host {
    fn new() -> Host {
        // Under /tmp: a socket's path has to fit in about a hundred bytes.
        let dir = tempfile::Builder::new()
            .prefix("perchd")
            .tempdir_in("/tmp")
            .unwrap();
        let host = Host { dir };
        for sub in ["perch", "home", "bin"] {
            std::fs::create_dir_all(host.dir.path().join(sub)).unwrap();
        }
        // Found ahead of any real gh, and knowing no token.
        host.script("bin/gh", "#!/bin/sh\nexit 1\n");
        std::fs::write(
            host.perch().join("config.toml"),
            "[agent]\nbinary = \"/nonexistent/claude\"\n\n\
             [agent.codex]\nbinary = \"/nonexistent/codex\"\n\n\
             [agent.gemini]\nbinary = \"/nonexistent/agy\"\n",
        )
        .unwrap();
        // ssh: its last argument is what the host's login shell runs.
        let ssh = host.script(
            "ssh",
            &format!(
                "#!/bin/sh\nfor last; do :; done\nexec env -i HOME={home} PERCH_HOME={perch} \
                 PATH={bin}:/usr/bin:/bin SHELL=/bin/sh RUST_LOG=perch=info,warn \
                 /bin/sh -c \"$last\"\n",
                home = host.dir.path().join("home").display(),
                perch = host.perch().display(),
                bin = host.dir.path().join("bin").display(),
            ),
        );
        std::env::set_var("PERCH_SSH", ssh);
        host
    }

    fn script(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// The host's `$PERCH_HOME`.
    fn perch(&self) -> PathBuf {
        self.dir.path().join("perch")
    }

    /// The engines the host keeps, by name.
    /// The builds kept in the home's `bin`, leaving out the link to the
    /// running one.
    fn kept(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.perch().join("bin"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "perchd")
            .collect();
        names.sort();
        names
    }

    fn engine_pid(&self) -> Option<i32> {
        EngineLock::holder(&self.perch())
            .flatten()
            .and_then(|pid| i32::try_from(pid).ok())
    }
}

/// The engine the test started is stopped whatever the test did.
impl Drop for Host {
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

/// An app that carries `binary` as its engine for this machine, and none
/// for any other.
fn app_with(binary: &[u8]) -> Source {
    let here = Platform::here().expect("a platform perchd is built for");
    Source {
        carried: vec![Carried::pack(here, binary)],
        workspace: None,
    }
}

/// Connect as `perch --connect` does, as `app`, and give what it
/// connected to and what it said on the way.
async fn connect(app: &Source) -> (Connected, Vec<String>) {
    let said = Arc::new(Mutex::new(Vec::new()));
    let heard = said.clone();
    let connected = connect_once(
        "halo",
        Bridge::Installed(app.clone()),
        PATIENCE,
        move |line| heard.lock().unwrap().push(line.to_string()),
    )
    .await
    .expect("connected");
    let said = said.lock().unwrap().clone();
    (connected, said)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_brings_its_engine_to_a_host_and_starts_it() {
    let host = Host::new();
    let perchd = std::fs::read(PERCHD).unwrap();
    let app = app_with(&perchd);
    let build = provision::build_of(&perchd);
    assert!(host.kept().is_empty());

    // The host has no engine of the app's: it is sent, kept under its
    // hash, and started.
    let (first, said) = connect(&app).await;
    assert_eq!(first.welcome.build, build);
    assert_eq!(first.other, None);
    assert!(
        said.iter().any(|line| line.starts_with("Sending perchd ")),
        "{said:?}"
    );
    assert_eq!(host.kept(), [format!("perchd-{build}")]);
    let pid = host.engine_pid().expect("an engine holds the home");
    // `perchd context` on the host is the running engine's build.
    let linked = std::fs::read_link(host.perch().join("bin/perchd")).unwrap();
    assert_eq!(
        linked.file_name().unwrap().to_string_lossy(),
        format!("perchd-{build}")
    );

    // Again: the host has it and it is running, so nothing is sent and
    // nothing is started.
    let (again, said) = connect(&app).await;
    assert_eq!(again.welcome.build, build);
    assert!(said.is_empty(), "{said:?}");
    assert_eq!(host.engine_pid(), Some(pid));
    assert_eq!(host.kept(), [format!("perchd-{build}")]);

    // An app that carries another build: the same engine with a byte
    // after it, which runs the same and hashes otherwise. Its engine is
    // sent beside the first, and the first, with nothing running and
    // nobody connected, makes way for it.
    let mut rebuilt = perchd.clone();
    rebuilt.push(0);
    let next = provision::build_of(&rebuilt);
    assert_ne!(next, build);
    let (other, said) = connect(&app_with(&rebuilt)).await;
    assert_eq!(other.welcome.build, next);
    assert_eq!(other.other, None);
    assert!(
        said.iter()
            .any(|line| line.starts_with("Restarting halo's engine")),
        "{said:?}"
    );
    let mut both = [format!("perchd-{build}"), format!("perchd-{next}")];
    both.sort();
    assert_eq!(host.kept(), both);
    assert_ne!(host.engine_pid(), Some(pid));

    // And the first app's is still there for it to come back to.
    let (back, said) = connect(&app).await;
    assert_eq!(back.welcome.build, build);
    assert!(
        !said.iter().any(|line| line.starts_with("Sending perchd ")),
        "{said:?}"
    );
}
