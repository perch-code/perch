//! Installing a missing server, from a click.
//!
//! An install into Perch's own directory writes to a `.partial` sibling
//! and is renamed into place only once it has succeeded, so a crash or a
//! quit never leaves half a server where `find` looks.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::server::{partial, Install};

/// How long an install may take before it's given up.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Run `install` to the end. The error is the line that says what went
/// wrong, as an exiting server's is.
pub async fn install(install: &Install) -> Result<(), String> {
    install_within(install, INSTALL_TIMEOUT).await
}

pub(crate) async fn install_within(install: &Install, timeout: Duration) -> Result<(), String> {
    match install {
        Install::Command {
            program,
            args,
            env,
            into,
        } => {
            let staging = into.as_deref().map(partial);
            if let Some(staging) = &staging {
                fresh_dir(staging).await?;
            }
            let ran = run(program, args, env, timeout).await;
            match (ran, into, staging) {
                (Ok(()), Some(into), Some(staging)) => settle(&staging, into).await,
                (Ok(()), _, _) => Ok(()),
                (Err(e), _, staging) => {
                    if let Some(staging) = staging {
                        let _ = tokio::fs::remove_dir_all(&staging).await;
                    }
                    Err(e)
                }
            }
        }
    }
}

/// An empty directory at `dir`, whatever was there.
async fn fresh_dir(dir: &Path) -> Result<(), String> {
    let _ = tokio::fs::remove_dir_all(dir).await;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("could not create {}: {e}", dir.display()))
}

/// Move a finished install from `staging` to `into`, over an older copy
/// of the same version.
async fn settle(staging: &Path, into: &Path) -> Result<(), String> {
    let _ = tokio::fs::remove_dir_all(into).await;
    tokio::fs::rename(staging, into)
        .await
        .map_err(|e| format!("could not move the install into {}: {e}", into.display()))
}

/// Run a command, logging what it writes, and fail with its error line.
async fn run(
    program: &Path,
    args: &[String],
    env: &[(String, String)],
    timeout: Duration,
) -> Result<(), String> {
    let name = program
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        // As everywhere a child starts: a pin from `cargo run` would pick
        // the wrong toolchain.
        .env_remove("RUSTUP_TOOLCHAIN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not run {name}: {e}"))?;
    let tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let mut readers = Vec::new();
    for stream in [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let (tail, name) = (tail.clone(), name.clone());
        readers.push(tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(install = %name, "{line}");
                if let Ok(mut tail) = tail.lock() {
                    if tail.len() == 40 {
                        tail.remove(0);
                    }
                    tail.push(line);
                }
            }
        }));
    }
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(format!("{name} failed: {e}")),
        Err(_) => {
            let _ = child.kill().await;
            return Err(format!("{name} took longer than {} minutes", timeout.as_secs() / 60));
        }
    };
    for reader in readers {
        let _ = reader.await;
    }
    if status.success() {
        return Ok(());
    }
    let said = tail
        .lock()
        .ok()
        .and_then(|tail| crate::client::exit_reason(tail.iter().map(String::as_str)));
    Err(said.unwrap_or_else(|| format!("{name} exited with {status}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sh(script: &str, into: Option<PathBuf>) -> Install {
        Install::Command {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script.into()],
            env: vec![("PERCH_TEST".into(), "yes".into())],
            into,
        }
    }

    /// A finished install is renamed into place; a failed one leaves no
    /// directory behind and says its error line.
    #[tokio::test]
    async fn an_install_lands_whole_or_not_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let into = dir.path().join("gopls/0.23.0");
        let staging = partial(&into);
        let ok = sh(
            &format!("test \"$PERCH_TEST\" = yes && touch {}/gopls", staging.display()),
            Some(into.clone()),
        );
        install(&ok).await.unwrap();
        assert!(into.join("gopls").is_file());
        assert!(!staging.exists());

        let other = dir.path().join("gopls/0.24.0");
        let failing = sh(
            &format!(
                "touch {}/half; echo 'note: fetching'; echo 'error: no network' >&2; exit 3",
                partial(&other).display()
            ),
            Some(other.clone()),
        );
        assert_eq!(install(&failing).await.unwrap_err(), "no network");
        assert!(!other.exists());
        assert!(!partial(&other).exists());
    }

    #[tokio::test]
    async fn an_install_that_hangs_is_given_up() {
        let hang = sh("sleep 30", None);
        let error = install_within(&hang, Duration::from_millis(200)).await.unwrap_err();
        assert!(error.contains("took longer"), "{error}");
    }
}
