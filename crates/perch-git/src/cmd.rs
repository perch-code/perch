//! Running the `git` binary.
//!
//! Network and store-mutating operations shell out rather than going through
//! a library: `git` handles every transport and authentication edge case
//! already, and it is the binary the user has configured and trusts.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::Error;

/// Environment variable the credential helper reads the token from.
///
/// The token travels in the child's environment, never in argv (visible to
/// `ps`) and never written to a config file.
const TOKEN_ENV: &str = "PERCH_GIT_TOKEN";

/// A credential helper that answers with the token from the environment,
/// for https://github.com and no other host.
///
/// The leading empty `credential.helper=` clears helpers inherited from the
/// user's global config, scoped ones included, so the macOS keychain never
/// gets a look in and cannot pop a dialog during a headless fetch. The
/// empty scoped one clears github.com's again, such as `gh auth
/// setup-git`'s, for good measure.
///
/// The token's helper is scoped because git hands `-c` config down to the
/// programs it runs, git-lfs among them, and a pull request's `.lfsconfig`
/// can point git-lfs at any host. git asks the helpers for that host's
/// credentials, and an unscoped helper would answer with the token. The
/// scope has to match scheme, host and port, so `http://github.com` and
/// `https://github.com.evil.example` get nothing, and `Git::command` turns
/// askpass off so the reader is not asked instead. Perch's own git only
/// talks to github.com (`RepoId::clone_url`); an enterprise remote is
/// fetched with the reader's git.
fn credential_args() -> [String; 6] {
    [
        "-c".into(),
        "credential.helper=".into(),
        "-c".into(),
        "credential.https://github.com.helper=".into(),
        "-c".into(),
        format!(
            "credential.https://github.com.helper=!f() {{ echo username=x-access-token; echo password=${TOKEN_ENV}; }}; f"
        ),
    ]
}

/// The arguments with `--ignore-submodules=dirty` after a `status` or
/// `diff` subcommand. For a submodule whose directory holds a `.git`,
/// either one otherwise runs `git status` in there, under that
/// repository's config: its `core.fsmonitor` and its filters run as
/// Perch. A Work run can put a `.git` there, by moving in a directory
/// that holds one, which no sandbox rule catches. With the flag git only
/// compares the submodule's `HEAD` with the commit recorded for it, so a
/// submodule with edits and no new commit no longer shows as changed.
/// `-c diff.ignoreSubmodules=dirty` would not do: a `.gitmodules` in the
/// worktree outranks it.
fn guard_submodules<I, S>(args: I) -> Vec<std::ffi::OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut args: Vec<std::ffi::OsString> =
        args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    // Past git's own options, some of which take the next argument.
    let mut i = 0;
    while let Some(arg) = args
        .get(i)
        .and_then(|a| a.to_str())
        .filter(|a| a.starts_with('-'))
    {
        i += if matches!(
            arg,
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace"
        ) {
            2
        } else {
            1
        };
    }
    if args
        .get(i)
        .is_some_and(|sub| sub == "status" || sub == "diff")
    {
        args.insert(i + 1, "--ignore-submodules=dirty".into());
    }
    args
}

#[derive(Debug, Clone)]
pub struct Git {
    /// Shared by every clone, so a token taken up after launch reaches
    /// the git commands already in flight from the same `Preparer`.
    token: Arc<RwLock<Option<String>>>,
    /// The reader's own git, for a command in one of their clones: their
    /// credential helpers and none of Perch's token.
    plain: bool,
}

impl Git {
    pub fn new(token: Option<String>) -> Self {
        Self {
            token: Arc::new(RwLock::new(token)),
            plain: false,
        }
    }

    /// Git as the reader has it set up, for a fetch in one of their own
    /// clones: whatever credential helper and SSH key their remotes use,
    /// never Perch's token. It still cannot prompt: git's terminal prompt
    /// is off, and stdin is empty, so an SSH key that wants a passphrase
    /// fails the command rather than hanging it. Callers that start SSH
    /// add `BatchMode` to it, see `Preparer::fetch_branch`.
    pub fn plain() -> Self {
        Self {
            token: Arc::new(RwLock::new(None)),
            plain: true,
        }
    }

    /// Use this token from the next command on. The one GitHub rotated
    /// under the app is no good to git either.
    pub fn set_token(&self, token: Option<String>) {
        if let Ok(mut current) = self.token.write() {
            *current = token;
        }
    }

    fn token(&self) -> Option<String> {
        self.token.read().ok().and_then(|token| token.clone())
    }

    /// Run git and return stdout, failing on a non-zero exit.
    pub async fn run<I, S>(
        &self,
        cwd: Option<&Path>,
        args: I,
        timeout: Duration,
    ) -> Result<String, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_env(cwd, args, &[], timeout).await
    }

    /// [`run`](Self::run) with variables added to git's environment, such
    /// as `GIT_INDEX_FILE` for work on an index of Perch's own.
    pub async fn run_env<I, S>(
        &self,
        cwd: Option<&Path>,
        args: I,
        env: &[(&str, &OsStr)],
        timeout: Duration,
    ) -> Result<String, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(cwd, args);
        for (name, value) in env {
            command.env(name, value);
        }
        let stdout = stdout(command, timeout).await?;
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    /// Run git and preserve stdout byte-for-byte. Review snapshot diffs and
    /// blobs may be binary and must not pass through lossy UTF-8 conversion.
    pub async fn run_bytes<I, S>(
        &self,
        cwd: Option<&Path>,
        args: I,
        timeout: Duration,
    ) -> Result<Vec<u8>, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        stdout(self.command(cwd, args), timeout).await
    }

    /// Run git with a bounded stdin payload. This is used by `pack-objects`
    /// so an exact set of already-resolved commits can be retained without
    /// creating or moving any ref in the reader's repository.
    pub async fn run_with_input<I, S>(
        &self,
        cwd: Option<&Path>,
        args: I,
        input: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(cwd, args);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| Error::Spawn(e.to_string()))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Spawn("git stdin was not piped".into()))?;
        let input = input.to_vec();

        let output = tokio::time::timeout(timeout, async move {
            stdin
                .write_all(&input)
                .await
                .map_err(|e| Error::Spawn(e.to_string()))?;
            stdin
                .shutdown()
                .await
                .map_err(|e| Error::Spawn(e.to_string()))?;
            // `pack-objects --revs` does not start producing output until it
            // observes EOF. Keep no live ChildStdin handle while awaiting the
            // child: on macOS, `shutdown` alone does not reliably deliver
            // that EOF before the handle is dropped.
            drop(stdin);
            child
                .wait_with_output()
                .await
                .map_err(|e| Error::Spawn(e.to_string()))
        })
        .await
        .map_err(|_| Error::Timeout)??;
        stdout_of(output)
    }

    /// Run git, forwarding `--progress` output as it arrives.
    ///
    /// git writes progress to stderr with carriage returns rather than
    /// newlines, so the stream is split on both.
    ///
    /// `idle` is how long git may go without a word before it is taken
    /// for stuck and killed. There is no limit on the whole run: a large
    /// repository clones for as long as it keeps moving.
    pub async fn run_with_progress<I, S>(
        &self,
        cwd: Option<&Path>,
        args: I,
        idle: Duration,
        on_progress: impl FnMut(Progress) + Send,
    ) -> Result<(), Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_with_progress_input(cwd, args, None, idle, on_progress)
            .await
    }

    /// `run_with_progress`, with `input` written to git's stdin.
    pub async fn run_with_progress_input<I, S>(
        &self,
        cwd: Option<&Path>,
        args: I,
        input: Option<Vec<u8>>,
        idle: Duration,
        mut on_progress: impl FnMut(Progress) + Send,
    ) -> Result<(), Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(cwd, args);
        command.stdout(Stdio::null()).stderr(Stdio::piped());
        if input.is_some() {
            command.stdin(Stdio::piped());
        }

        let mut child = command.spawn().map_err(|e| Error::Spawn(e.to_string()))?;
        let stderr = child.stderr.take().expect("stderr piped");
        // Written beside the reading, so a large input cannot fill the pipe
        // while git waits for its stderr to be read.
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            tokio::spawn(async move {
                let _ = stdin.write_all(&input).await;
                let _ = stdin.shutdown().await;
            });
        }

        let mut reader = BufReader::new(stderr);
        let mut buf = Vec::new();
        let mut tail = String::new();
        loop {
            buf.clear();
            // Progress lines end in \r; the final summary ends in \n.
            let read = match tokio::time::timeout(idle, reader.read_until(b'\r', &mut buf)).await {
                Ok(read) => read.unwrap_or(0),
                Err(_) => {
                    let _ = child.start_kill();
                    return Err(Error::Timeout);
                }
            };
            if read == 0 {
                break;
            }
            let chunk = String::from_utf8_lossy(&buf);
            for line in chunk.split(['\r', '\n']) {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                tail = line.to_string();
                on_progress(parse_progress(line));
            }
        }

        let status = match tokio::time::timeout(idle, child.wait()).await {
            Ok(status) => status.map_err(|e| Error::Spawn(e.to_string()))?,
            Err(_) => {
                let _ = child.start_kill();
                return Err(Error::Timeout);
            }
        };
        if !status.success() {
            return Err(Error::Git {
                status: status.code().unwrap_or(-1),
                stderr: tail,
            });
        }
        Ok(())
    }

    fn command<I, S>(&self, cwd: Option<&Path>, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("git");
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        if !self.plain {
            command.args(credential_args());
        }
        command.args(guard_submodules(args));
        // Never let git block on an interactive prompt inside a GUI app.
        command.env("GIT_TERMINAL_PROMPT", "0");
        command.stdin(Stdio::null());
        // The system config is where Apple's git keeps the keychain
        // helper, which the reader's own fetches need and Perch's do not.
        if !self.plain {
            command.env("GIT_CONFIG_NOSYSTEM", "1");
            // Asked about a host the token's helper skips, such as one a
            // pull request's `.lfsconfig` names, git and git-lfs fall back
            // to askpass: `GIT_ASKPASS`, `core.askPass`, then
            // `SSH_ASKPASS`. That pops a dialog whose answer goes to the
            // host. Set but empty, `GIT_ASKPASS` wins and runs nothing, so
            // the request fails instead.
            command.env("GIT_ASKPASS", "");
        }
        // Some of git's messages are read back as text; a translated git
        // would say them differently.
        command.env("LC_ALL", "C");
        if let Some(token) = self.token().filter(|_| !self.plain) {
            command.env(TOKEN_ENV, token);
        }
        command.kill_on_drop(true);
        command
    }
}

/// Run `command` to its end and return its stdout, failing on a timeout
/// or a non-zero exit.
async fn stdout(mut command: Command, timeout: Duration) -> Result<Vec<u8>, Error> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|e| Error::Spawn(e.to_string()))?;
    stdout_of(output)
}

/// The stdout of a git that exited zero, or its status and stderr.
fn stdout_of(output: std::process::Output) -> Result<Vec<u8>, Error> {
    if !output.status.success() {
        return Err(Error::Git {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output.stdout)
}

/// A parsed line of git's progress output.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// The phase git named, such as "Receiving objects".
    pub phase: String,
    pub percent: Option<u8>,
}

fn parse_progress(line: &str) -> Progress {
    // "Receiving objects:  42% (123/456), 1.2 MiB | 3.4 MiB/s"
    let (phase, rest) = match line.split_once(':') {
        Some((p, r)) => (p.trim().to_string(), r),
        None => {
            return Progress {
                phase: line.to_string(),
                percent: None,
            }
        }
    };

    let percent = rest
        .split('%')
        .next()
        .and_then(|s| s.trim().rsplit(|c: char| !c.is_ascii_digit()).next())
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u8>().ok())
        .filter(|_| rest.contains('%'))
        .map(|p| p.min(100));

    Progress { phase, percent }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_diff_leave_the_inside_of_submodules_alone() {
        let guarded = |args: &[&str]| -> Vec<String> {
            guard_submodules(args)
                .into_iter()
                .map(|a| a.into_string().unwrap())
                .collect()
        };
        assert_eq!(
            guarded(&["status", "--porcelain"]),
            ["status", "--ignore-submodules=dirty", "--porcelain"]
        );
        assert_eq!(
            guarded(&["diff", "HEAD"]),
            ["diff", "--ignore-submodules=dirty", "HEAD"]
        );
        assert_eq!(
            guarded(&["--no-optional-locks", "status"]),
            ["--no-optional-locks", "status", "--ignore-submodules=dirty"]
        );
        assert_eq!(
            guarded(&["-c", "diff.foo=status", "-C", "diff", "diff"]),
            [
                "-c",
                "diff.foo=status",
                "-C",
                "diff",
                "diff",
                "--ignore-submodules=dirty"
            ]
        );
        assert_eq!(guarded(&["log", "--", "status"]), ["log", "--", "status"]);
        assert_eq!(guarded(&["show", "diff"]), ["show", "diff"]);
        assert!(guarded(&[]).is_empty());
    }

    #[test]
    fn progress_lines_yield_a_phase_and_percentage() {
        let p = parse_progress("Receiving objects:  42% (123/456), 1.2 MiB | 3.4 MiB/s");
        assert_eq!(p.phase, "Receiving objects");
        assert_eq!(p.percent, Some(42));
    }

    #[test]
    fn progress_at_completion_reads_as_one_hundred() {
        let p = parse_progress("Resolving deltas: 100% (789/789), done.");
        assert_eq!(p.phase, "Resolving deltas");
        assert_eq!(p.percent, Some(100));
    }

    #[test]
    fn lines_without_a_percentage_still_name_their_phase() {
        let p = parse_progress("remote: Enumerating objects");
        assert_eq!(p.percent, None);
        assert!(p.phase.contains("Enumerating") || p.phase.contains("remote"));
    }

    #[test]
    fn a_line_with_no_colon_is_kept_whole() {
        let p = parse_progress("Cloning into bare repository");
        assert_eq!(p.phase, "Cloning into bare repository");
        assert_eq!(p.percent, None);
    }

    #[test]
    fn the_credential_helper_reads_the_token_from_the_environment() {
        let args = credential_args();
        // The first helpers are empty, which clears inherited ones.
        assert_eq!(args[1], "credential.helper=");
        assert_eq!(args[3], "credential.https://github.com.helper=");
        assert!(args[5].starts_with("credential.https://github.com.helper=!"));
        assert!(args[5].contains(TOKEN_ENV));
        // The token itself must never appear in a command line.
        assert!(!args[5].contains("ghp_"));
    }

    const TOKEN: &str = "perch-test-token";

    /// What askpass answers in the tests, so the output shows whether git
    /// fell back to asking the reader.
    const ASKED: &str = "askpass-ran";

    /// What Perch's git says to `git credential fill` for `request`, on a
    /// machine whose only global config is `config`. `core.askPass` and
    /// `SSH_ASKPASS` both name a program that answers [`ASKED`].
    async fn credential_fill(config: &str, request: &str) -> String {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let askpass = home.path().join("askpass");
        std::fs::write(&askpass, format!("#!/bin/sh\necho {ASKED}\n")).unwrap();
        std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o755)).unwrap();
        let global = home.path().join("gitconfig");
        std::fs::write(
            &global,
            format!("[core]\n\taskPass = \"{}\"\n{config}", askpass.display()),
        )
        .unwrap();

        let mut command =
            Git::new(Some(TOKEN.into())).command(Some(home.path()), ["credential", "fill"]);
        // Whatever GIT_ASKPASS Perch was started with, git gets an empty one.
        assert!(command
            .as_std()
            .get_envs()
            .any(|(name, value)| name == "GIT_ASKPASS" && value == Some(OsStr::new(""))));
        command
            .env("GIT_CONFIG_GLOBAL", &global)
            .env("HOME", home.path())
            .env("SSH_ASKPASS", &askpass)
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_COUNT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("git runs");
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(request.as_bytes()).await.unwrap();
        drop(stdin);
        let output = child.wait_with_output().await.unwrap();
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    #[tokio::test]
    async fn the_token_goes_to_github_com_and_nowhere_else() {
        let said = credential_fill("", "protocol=https\nhost=github.com\n\n").await;
        assert!(said.contains(&format!("password={TOKEN}")), "{said}");
        assert!(!said.contains(ASKED), "{said}");

        for request in [
            // A pull request's .lfsconfig naming its own server.
            "protocol=https\nhost=attacker.example\n\n",
            "protocol=http\nhost=github.com\n\n",
            "protocol=https\nhost=github.com.attacker.example\n\n",
            "protocol=https\nhost=github.com:8443\n\n",
        ] {
            let said = credential_fill("", request).await;
            assert!(!said.contains(TOKEN), "{request:?} got the token: {said}");
            // Nor does the reader get asked for that host's password.
            assert!(!said.contains(ASKED), "{request:?} ran askpass: {said}");
        }
    }

    #[tokio::test]
    async fn inherited_helpers_answer_for_no_host() {
        // A helper with a whole answer, so git would ask no later one.
        let helper = "\"!f() { echo username=inherited; echo password=inherited; }; f\"";
        let config = format!("[credential]\n\thelper = {helper}\n[credential \"https://github.com\"]\n\thelper = {helper}\n");

        let said = credential_fill(&config, "protocol=https\nhost=github.com\n\n").await;
        assert!(said.contains(&format!("password={TOKEN}")), "{said}");
        assert!(!said.contains("inherited"), "{said}");

        let said = credential_fill(&config, "protocol=https\nhost=attacker.example\n\n").await;
        assert!(!said.contains(TOKEN), "{said}");
        assert!(!said.contains("inherited"), "{said}");
        assert!(!said.contains(ASKED), "{said}");
    }

    /// git as a shell alias, so a test can say when it speaks.
    fn alias(script: &str) -> [String; 3] {
        [
            "-c".to_string(),
            format!("alias.talk=!{script}"),
            "talk".to_string(),
        ]
    }

    #[tokio::test]
    async fn progress_keeps_a_long_run_alive() {
        let mut seen = 0;
        let result = Git::plain()
            .run_with_progress(
                None,
                alias("for i in 1 2 3 4 5 6; do printf 'Receiving objects: %d%%\\r' $i >&2; sleep 0.2; done"),
                Duration::from_millis(600),
                |_| seen += 1,
            )
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(seen, 6);
    }

    #[tokio::test]
    async fn a_silent_run_is_stopped() {
        let started = std::time::Instant::now();
        let result = Git::plain()
            .run_with_progress(
                None,
                alias("printf 'Receiving objects: 1%%\\r' >&2; sleep 5"),
                Duration::from_millis(300),
                |_| {},
            )
            .await;
        assert!(matches!(result, Err(Error::Timeout)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
