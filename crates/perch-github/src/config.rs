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
    #[serde(default)]
    pub knowledge: KnowledgeConfig,
}

/// Where archived discussion goes and which repositories share a project.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct KnowledgeConfig {
    /// Defaults to `~/.perch/knowledge`. A leading `~` is the home directory.
    #[serde(default)]
    pub root: Option<String>,
    /// Write the two `SKILL.md` files. On unless turned off.
    #[serde(default = "default_true")]
    pub skills: bool,
    /// `[[knowledge.project]]` entries.
    #[serde(default)]
    pub project: Vec<ProjectConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ProjectConfig {
    pub name: String,
    /// `owner/repo` names that belong to this project.
    #[serde(default)]
    pub repos: Vec<String>,
}

fn default_true() -> bool {
    true
}

// By hand rather than derived: an absent `[knowledge]` table must mean the
// same as an empty one, skills on.
impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self { root: None, skills: true, project: Vec::new() }
    }
}

impl KnowledgeConfig {
    /// The knowledge root, `~` expanded.
    pub fn root_path(&self) -> PathBuf {
        match &self.root {
            Some(root) => expand_home(root),
            None => home_dir().join(".perch").join("knowledge"),
        }
    }

    /// The project a repository belongs to: the configured one, or the
    /// repository's own name, so nothing has to be configured before the
    /// first file appears.
    pub fn project_for(&self, repo_full_name: &str) -> String {
        self.project
            .iter()
            .find(|p| p.repos.iter().any(|r| r.eq_ignore_ascii_case(repo_full_name)))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| {
                repo_full_name
                    .rsplit('/')
                    .next()
                    .unwrap_or(repo_full_name)
                    .to_string()
            })
    }
}

pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home_dir().join(rest),
        None if path == "~" => home_dir(),
        None => PathBuf::from(path),
    }
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
            knowledge: KnowledgeConfig::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_config_still_parses() {
        let c: Config = toml::from_str("poll_interval_secs = 30").unwrap();
        assert_eq!(c.poll_interval_secs, 30);
        assert!(c.knowledge.skills);
        assert!(c.knowledge.project.is_empty());
        assert!(c.knowledge.root_path().ends_with(".perch/knowledge"));
    }

    #[test]
    fn projects_map_repositories_and_fall_back_to_the_repository_name() {
        let c: Config = toml::from_str(
            r#"
            [knowledge]
            root = "~/elsewhere"
            skills = false

            [[knowledge.project]]
            name = "slatedb"
            repos = ["slatedb/slatedb", "slatedb/slatedb-go"]
            "#,
        )
        .unwrap();
        assert_eq!(c.knowledge.project_for("slatedb/slatedb-go"), "slatedb");
        assert_eq!(c.knowledge.project_for("criccomini/perch"), "perch");
        assert!(!c.knowledge.skills);
        assert_eq!(c.knowledge.root_path(), home_dir().join("elsewhere"));
    }
}
