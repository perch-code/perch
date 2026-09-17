//! Config file and on-disk paths.

use std::path::PathBuf;

use serde::Deserialize;

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "perch";
const APPLICATION: &str = "perch";

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    /// Last-resort token, used only when env vars and `gh` come up empty.
    #[serde(default)]
    pub token: Option<String>,
    /// Floor for the poll interval. `X-Poll-Interval` may raise it, never lower it.
    #[serde(default = "default_poll_secs")]
    pub poll_interval_secs: u64,
}

fn default_poll_secs() -> u64 {
    60
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::fallback();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::fallback();
        };
        match toml::from_str::<Config>(&text) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "ignoring malformed config");
                Self::fallback()
            }
        }
    }

    fn fallback() -> Self {
        Config {
            token: None,
            poll_interval_secs: default_poll_secs(),
        }
    }
}

fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

pub fn config_path() -> Option<PathBuf> {
    Some(project_dirs()?.data_dir().join("config.toml"))
}

/// Directory for the SQLite database and the avatar cache.
pub fn data_dir() -> Option<PathBuf> {
    let dir = project_dirs()?.data_dir().to_path_buf();
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

pub fn db_path() -> Option<PathBuf> {
    Some(data_dir()?.join("perch.db"))
}

pub fn avatar_dir() -> Option<PathBuf> {
    let dir = data_dir()?.join("avatars");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}
