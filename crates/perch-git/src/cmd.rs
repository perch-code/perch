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

/// A credential helper that answers with the token from the environment.
///
/// The leading empty `credential.helper=` clears helpers inherited from the
/// user's global config, so the macOS keychain never gets a look in and
/// cannot pop a dialog during a headless fetch.
fn credential_args() -> [String; 4] {
    [
        "-c".into(),
        "credential.helper=".into(),
        "-c".into(),
        format!(
            "credential.helper=!f() {{ echo username=x-access-token; echo password=${TOKEN_ENV}; }}; f"
        ),
    ]
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
        let mut command = self.command(cwd);
        command.args(args);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = tokio::time::timeout(timeout, command.output())
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|e| Error::Spawn(e.to_string()))?;

        if !output.status.success() {
            return Err(Error::Git {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
        let mut command = self.command(cwd);
        command.args(args);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = tokio::time::timeout(timeout, command.output())
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|e| Error::Spawn(e.to_string()))?;
        if !output.status.success() {
            return Err(Error::Git {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(output.stdout)
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
        let mut command = self.command(cwd);
        command.args(args);
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

        if !output.status.success() {
            return Err(Error::Git {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(output.stdout)
    }

    /// Run git, forwarding `--progress` output as it arrives.
    ///
    /// git writes progress to stderr with carriage returns rather than
    /// newlines, so the stream is split on both.
    pub async fn run_with_progress<I, S>(
        &self,
        cwd: Option<&Path>,
        args: I,
        timeout: Duration,
        mut on_progress: impl FnMut(Progress) + Send,
    ) -> Result<(), Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(cwd);
        command.args(args);
        command.stdout(Stdio::null()).stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|e| Error::Spawn(e.to_string()))?;
        let stderr = child.stderr.take().expect("stderr piped");

        let pump = async {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::new();
            let mut tail = String::new();
            loop {
                buf.clear();
                // Progress lines end in \r; the final summary ends in \n.
                let read = reader.read_until(b'\r', &mut buf).await.unwrap_or(0);
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
            tail
        };

        let result = tokio::time::timeout(timeout, async {
            let tail = pump.await;
            let status = child
                .wait()
                .await
                .map_err(|e| Error::Spawn(e.to_string()))?;
            Ok::<_, Error>((status, tail))
        })
        .await;

        match result {
            Err(_) => {
                let _ = child.start_kill();
                Err(Error::Timeout)
            }
            Ok(Err(e)) => Err(e),
            Ok(Ok((status, tail))) if !status.success() => Err(Error::Git {
                status: status.code().unwrap_or(-1),
                stderr: tail,
            }),
            Ok(Ok(_)) => Ok(()),
        }
    }

    fn command(&self, cwd: Option<&Path>) -> Command {
        let mut command = Command::new("git");
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        if !self.plain {
            command.args(credential_args());
        }
        // Never let git block on an interactive prompt inside a GUI app.
        command.env("GIT_TERMINAL_PROMPT", "0");
        command.stdin(Stdio::null());
        // The system config is where Apple's git keeps the keychain
        // helper, which the reader's own fetches need and Perch's do not.
        if !self.plain {
            command.env("GIT_CONFIG_NOSYSTEM", "1");
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
        // The first helper is empty, which clears inherited ones.
        assert_eq!(args[1], "credential.helper=");
        assert!(args[3].contains(TOKEN_ENV));
        // The token itself must never appear in a command line.
        assert!(!args[3].contains("ghp_"));
    }
}
