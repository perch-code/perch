//! Token resolution.
//!
//! A GUI app launched from Finder does not inherit the shell environment, so
//! the env vars are usually empty in a real launch. The `gh` subprocess step
//! is what makes the app work when double-clicked, because `gh` keeps its
//! token in its own config rather than the environment.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    GithubTokenEnv,
    GhTokenEnv,
    GhCli,
    ConfigFile,
}

impl TokenSource {
    pub fn describe(self) -> &'static str {
        match self {
            TokenSource::GithubTokenEnv => "GITHUB_TOKEN",
            TokenSource::GhTokenEnv => "GH_TOKEN",
            TokenSource::GhCli => "gh auth token",
            TokenSource::ConfigFile => "config.toml",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub value: String,
    pub source: TokenSource,
}

/// First hit wins: `GITHUB_TOKEN`, `GH_TOKEN`, `gh auth token`, config file.
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
    None
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
}
