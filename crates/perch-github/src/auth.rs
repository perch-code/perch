//! Token resolution.
//!
//! A GUI app launched from Finder does not inherit the shell environment, so
//! the env vars are usually empty in a real launch. The `gh` subprocess step
//! is what makes the app work when double-clicked, because `gh` keeps its
//! token in its own config rather than the environment.

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
        return Some(Token { value, source: TokenSource::GithubTokenEnv });
    }
    if let Some(value) = non_empty_env("GH_TOKEN") {
        return Some(Token { value, source: TokenSource::GhTokenEnv });
    }
    if let Some(value) = gh_cli_token() {
        return Some(Token { value, source: TokenSource::GhCli });
    }
    if let Some(value) = config.token.as_ref().map(|t| t.trim().to_string()) {
        if !value.is_empty() {
            return Some(Token { value, source: TokenSource::ConfigFile });
        }
    }
    None
}

fn non_empty_env(key: &str) -> Option<String> {
    let v = std::env::var(key).ok()?.trim().to_string();
    (!v.is_empty()).then_some(v)
}

fn gh_cli_token() -> Option<String> {
    let out = Command::new("gh").args(["auth", "token"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let token = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!token.is_empty()).then_some(token)
}
