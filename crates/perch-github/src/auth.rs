//! Token resolution.
//!
//! A GUI app launched from Finder does not inherit the shell environment, so
//! the env vars are usually empty in a real launch. The `gh` subprocess step
//! is what makes the app work when double-clicked, because `gh` keeps its
//! token in its own config rather than the environment. A reader with no
//! `gh` who exported the token in a shell startup file is found last, by
//! asking the login shell what a terminal would have handed the app.

use std::ffi::OsStr;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    GithubTokenEnv,
    GhTokenEnv,
    GhCli,
    ConfigFile,
    /// `GITHUB_TOKEN` as the login shell's startup files export it.
    GithubTokenShell,
    /// `GH_TOKEN`, likewise.
    GhTokenShell,
}

impl TokenSource {
    pub fn describe(self) -> &'static str {
        match self {
            TokenSource::GithubTokenEnv => "GITHUB_TOKEN",
            TokenSource::GhTokenEnv => "GH_TOKEN",
            TokenSource::GhCli => "gh auth token",
            TokenSource::ConfigFile => "config.toml",
            TokenSource::GithubTokenShell => "GITHUB_TOKEN in the login shell",
            TokenSource::GhTokenShell => "GH_TOKEN in the login shell",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub value: String,
    pub source: TokenSource,
}

/// First hit wins: `GITHUB_TOKEN`, `GH_TOKEN`, `gh auth token`, config file,
/// then the two variables as the login shell exports them.
pub fn resolve_token(config: &Config) -> Option<Token> {
    if let Some(value) = non_empty_env("GITHUB_TOKEN") {
        return Some(Token {
            value,
            source: TokenSource::GithubTokenEnv,
        });
    }
    if let Some(value) = non_empty_env("GH_TOKEN") {
        return Some(Token {
            value,
            source: TokenSource::GhTokenEnv,
        });
    }
    if let Some(value) = gh_cli_token() {
        return Some(Token {
            value,
            source: TokenSource::GhCli,
        });
    }
    if let Some(value) = config.token.as_ref().map(|t| t.trim().to_string()) {
        if !value.is_empty() {
            return Some(Token {
                value,
                source: TokenSource::ConfigFile,
            });
        }
    }
    login_shell_token()
}

fn non_empty_env(key: &str) -> Option<String> {
    let v = std::env::var(key).ok()?.trim().to_string();
    (!v.is_empty()).then_some(v)
}

fn gh_cli_token() -> Option<String> {
    let gh = find_gh(std::env::var_os("PATH").as_deref(), &gh_install_dirs())?;
    let out = Command::new(gh).args(["auth", "token"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let token = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Where Homebrew, MacPorts and Nix put `gh`. A Finder launch gets only
/// `/usr/bin:/bin:/usr/sbin:/sbin` on `PATH`, which holds none of them.
fn gh_install_dirs() -> Vec<PathBuf> {
    let mut dirs = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"]
        .map(PathBuf::from)
        .to_vec();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".nix-profile/bin"));
        dirs.push(home.join(".local/bin"));
    }
    dirs
}

/// `gh` on `PATH`, else in the first of `extra` that has it.
fn find_gh(path: Option<&OsStr>, extra: &[PathBuf]) -> Option<PathBuf> {
    let on_path = path
        .into_iter()
        .flat_map(std::env::split_paths)
        .collect::<Vec<_>>();
    on_path
        .iter()
        .chain(extra)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join("gh"))
        .find(|gh| gh.is_file())
}

/// Printed either side of the shell's environment, so what a startup file
/// prints of its own is passed over.
const MARK: &str = "__perch_shell_env__";

/// How long the login shell gets, startup files and all.
const SHELL_WAIT: Duration = Duration::from_secs(5);

/// As much as is read of what the shell prints.
const SHELL_SAYS_AT_MOST: usize = 1024 * 1024;

/// The token the login shell exports, asked for once. Each ask runs the
/// startup files in full, and one that starts an agent or a daemon would
/// start another on every poll made without a token. So an export added
/// while Perch runs is found at the next launch, as one in Perch's own
/// environment would be.
fn login_shell_token() -> Option<Token> {
    static ASKED: OnceLock<Option<Token>> = OnceLock::new();
    ASKED
        .get_or_init(|| {
            let shell = non_empty_env("SHELL")?;
            let said = said_within(shell_command(Path::new(&shell)), SHELL_WAIT);
            let token = token_in(&said);
            tracing::info!(%shell, found = token.is_some(), "login shell asked for a token");
            token
        })
        .clone()
}

/// `shell` as a terminal starts it, login and interactive, so it reads
/// every startup file an export could be in, printing its environment
/// between two marks. `env` is asked rather than the shell's own `$NAME`
/// so that only what is exported counts, as it would for a child of that
/// shell, and so that no shell's syntax for a variable is needed.
fn shell_command(shell: &Path) -> Command {
    let mut command = Command::new(shell);
    command.args([
        "-l",
        "-i",
        "-c",
        &format!("printf {MARK}; /usr/bin/env; printf {MARK}"),
    ]);
    command
}

/// What `command` prints until it has printed both marks, or ends, or
/// `wait` is up. It is given no input and no terminal, and nothing it
/// started outlives the answer.
fn said_within(mut command: Command, wait: Duration) -> String {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // A session of its own. An interactive shell reaches for the terminal
    // it was started under and is stopped for it when Perch itself runs in
    // one; and the session is one process group, to stop as a whole.
    //
    // SAFETY: `setsid` is async-signal-safe, and reading `errno` neither
    // allocates nor locks, so both may run between `fork` and `exec`.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let Ok(mut child) = command.spawn() else {
        return String::new();
    };
    let said = child
        .stdout
        .take()
        .map(|mut stdout| read_marked(&mut stdout, Instant::now() + wait))
        .unwrap_or_default();
    // SAFETY: the child is not yet reaped, so its id still names it and
    // the group it leads, and nothing else.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.wait();
    String::from_utf8_lossy(&said).into_owned()
}

/// Read `stdout` until both marks are in, it ends, or `deadline` passes.
/// The end alone is not waited for: a job a startup file left running
/// holds the pipe open long after the shell has answered.
fn read_marked(stdout: &mut std::process::ChildStdout, deadline: Instant) -> Vec<u8> {
    let mark = MARK.as_bytes();
    let mut said = Vec::new();
    let mut chunk = [0_u8; 8192];
    // How many marks are in, and where the search for the next resumes.
    let (mut marks, mut from) = (0, 0);
    while marks < 2 && said.len() < SHELL_SAYS_AT_MOST {
        let left = deadline.saturating_duration_since(Instant::now());
        let mut poll = libc::pollfd {
            fd: stdout.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = libc::c_int::try_from(left.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: `poll` is one valid `pollfd`, alive for the call, on a
        // descriptor `stdout` keeps open.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        if ready < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if ready <= 0 {
            break;
        }
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => said.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        while let Some(at) = said[from..].windows(mark.len()).position(|w| w == mark) {
            marks += 1;
            from += at + mark.len();
        }
        // A mark may straddle this read and the next.
        from = from.max(said.len().saturating_sub(mark.len() - 1));
    }
    said
}

/// The token in the environment a shell printed between the marks:
/// `GITHUB_TOKEN`, else `GH_TOKEN`, as the process's own are taken.
fn token_in(said: &str) -> Option<Token> {
    let (_, rest) = said.split_once(MARK)?;
    let (env, _) = rest.rsplit_once(MARK)?;
    let exported = |key: &str| {
        env.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    if let Some(value) = exported("GITHUB_TOKEN") {
        return Some(Token {
            value,
            source: TokenSource::GithubTokenShell,
        });
    }
    exported("GH_TOKEN").map(|value| Token {
        value,
        source: TokenSource::GhTokenShell,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_off_a_bare_finder_path_is_found_in_an_install_dir() {
        let empty = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        std::fs::write(install.path().join("gh"), "").unwrap();
        let extra = [empty.path().to_path_buf(), install.path().to_path_buf()];
        let bare = OsStr::new("/nonexistent-a:/nonexistent-b");
        assert_eq!(find_gh(Some(bare), &extra), Some(install.path().join("gh")));
        assert_eq!(find_gh(Some(bare), &extra[..1]), None);
    }

    #[test]
    fn gh_on_path_wins_over_an_install_dir() {
        let on_path = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        std::fs::write(on_path.path().join("gh"), "").unwrap();
        std::fs::write(install.path().join("gh"), "").unwrap();
        let found = find_gh(
            Some(on_path.path().as_os_str()),
            &[install.path().to_path_buf()],
        );
        assert_eq!(found, Some(on_path.path().join("gh")));
    }

    /// What a shell printed around its environment, marks and all.
    fn said(env: &str) -> String {
        format!("Welcome back.\n{MARK}{env}{MARK}logout\n")
    }

    #[test]
    fn the_shells_token_is_read_past_what_its_startup_files_print() {
        let token = token_in(&said(
            "HOME=/Users/pat\nGITHUB_TOKEN=ghp_shell\nTERM=dumb\n",
        ))
        .unwrap();
        assert_eq!(token.value, "ghp_shell");
        assert_eq!(token.source, TokenSource::GithubTokenShell);
        assert_eq!(token.source.describe(), "GITHUB_TOKEN in the login shell");
    }

    #[test]
    fn the_shells_github_token_wins_over_its_gh_token() {
        let both = token_in(&said("GH_TOKEN=gho_second\nGITHUB_TOKEN=ghp_first\n")).unwrap();
        assert_eq!(both.value, "ghp_first");
        let one = token_in(&said("GITHUB_TOKEN=\nGH_TOKEN= gho_second \n")).unwrap();
        assert_eq!(one.value, "gho_second");
        assert_eq!(one.source, TokenSource::GhTokenShell);
    }

    #[test]
    fn a_shell_with_no_token_or_no_answer_gives_none() {
        assert!(token_in(&said("HOME=/Users/pat\nGITHUB_TOKEN_FILE=/tmp/t\n")).is_none());
        assert!(token_in(&said("MY_GITHUB_TOKEN=ghp_other\n")).is_none());
        // The startup files printed a token's line, but the shell never
        // got as far as its environment.
        assert!(token_in("GITHUB_TOKEN=ghp_printed\n").is_none());
        assert!(token_in(&format!("{MARK}GITHUB_TOKEN=ghp_cut_short\n")).is_none());
        assert!(token_in("").is_none());
    }

    /// `/bin/sh` reads `~/.profile` as a login shell and `$ENV` as an
    /// interactive one, as zsh does `.zprofile` and `.zshrc`.
    #[test]
    fn a_token_exported_in_a_startup_file_is_found() {
        let home = tempfile::tempdir().unwrap();
        let rc = home.path().join("rc");
        std::fs::write(
            home.path().join(".profile"),
            "echo Welcome back.\nexport GITHUB_TOKEN=ghp_profile\n",
        )
        .unwrap();
        std::fs::write(&rc, "export GH_TOKEN=gho_rc\n").unwrap();

        let mut command = shell_command(Path::new("/bin/sh"));
        command.env("HOME", home.path()).env("ENV", &rc);
        // What it said is the whole environment, so no failure prints it.
        let said = said_within(command, Duration::from_secs(30));
        let token = token_in(&said).expect("no token from ~/.profile");
        assert_eq!(token.value, "ghp_profile");
        assert_eq!(token.source, TokenSource::GithubTokenShell);
        assert!(said.contains("GH_TOKEN=gho_rc"), "$ENV was not read");

        std::fs::write(home.path().join(".profile"), "").unwrap();
        let mut command = shell_command(Path::new("/bin/sh"));
        command
            .env("HOME", home.path())
            .env("ENV", &rc)
            .env_remove("GITHUB_TOKEN");
        let token = token_in(&said_within(command, Duration::from_secs(30))).unwrap();
        assert_eq!(token.value, "gho_rc");
        assert_eq!(token.source, TokenSource::GhTokenShell);
    }

    #[test]
    fn a_shell_that_never_answers_is_given_up_on() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let started = Instant::now();
        assert_eq!(said_within(command, Duration::from_millis(200)), "");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(said_within(Command::new("/nonexistent/sh"), SHELL_WAIT), "");
    }

    /// A startup file that leaves a job running keeps the pipe open; the
    /// answer is taken when it is in, and the job goes with the shell.
    #[test]
    fn a_job_a_startup_file_left_running_does_not_hold_the_answer() {
        let dir = tempfile::tempdir().unwrap();
        let beat = dir.path().join("beat");
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            &format!(
                "(while :; do : > '{}'; sleep 0.05; done) & \
                 printf '{MARK}GITHUB_TOKEN=ghp_prompt\\n{MARK}'; wait",
                beat.display()
            ),
        ]);
        let started = Instant::now();
        let said = said_within(command, Duration::from_secs(30));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(token_in(&said).unwrap().value, "ghp_prompt");

        // The job wrote its file every twentieth of a second.
        let _ = std::fs::remove_file(&beat);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!beat.exists(), "the job outlived the shell");
    }
}
