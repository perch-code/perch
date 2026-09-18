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
    #[serde(default)]
    pub agent: AgentConfig,
    /// `[[project]]` entries.
    #[serde(default)]
    pub project: Vec<ProjectConfig>,
    /// Things wrong with the file that are worth one line in the status
    /// bar. Filled by `load`, never by the file.
    #[serde(skip)]
    pub warnings: Vec<String>,
}

/// The coding agent a thread's question goes to.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AgentConfig {
    /// The executable. Found on `PATH` unless a path is given here.
    #[serde(default = "default_binary")]
    pub binary: String,
    /// What the picker starts on.
    #[serde(default = "default_model")]
    pub model: String,
    /// `low`, `medium`, `high`, `xhigh` or `max`.
    #[serde(default = "default_effort")]
    pub effort: String,
    /// What the picker offers.
    #[serde(default = "default_models")]
    pub models: Vec<String>,
}

fn default_binary() -> String {
    "claude".into()
}
fn default_model() -> String {
    "sonnet".into()
}
fn default_effort() -> String {
    "medium".into()
}
fn default_models() -> Vec<String> {
    // The aliases the CLI resolves to the latest of each tier.
    vec!["fable".into(), "opus".into(), "sonnet".into(), "haiku".into()]
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            binary: default_binary(),
            model: default_model(),
            effort: default_effort(),
            models: default_models(),
        }
    }
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
    /// The old home of `[[project]]`. Read only to say it has moved.
    #[serde(default)]
    pub project: Vec<ProjectConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ProjectConfig {
    pub name: String,
    /// `owner/repo` names that belong to this project.
    #[serde(default)]
    pub repos: Vec<String>,
    /// Directories that are checkouts of one of those repositories, or
    /// worktrees of one. A leading `~` is the home directory.
    #[serde(default)]
    pub roots: Vec<String>,
}

impl ProjectConfig {
    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|r| expand_home(r)).collect()
    }
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

impl Config {
    /// The project a repository belongs to: the declared one, or the
    /// repository's own name, so nothing has to be configured before the
    /// first item appears. The knowledge directory uses the same rule, so
    /// the two agree.
    pub fn project_for(&self, repo_full_name: &str) -> String {
        self.project
            .iter()
            .find(|p| p.repos.iter().any(|r| r.eq_ignore_ascii_case(repo_full_name)))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| implicit_project_name(repo_full_name))
    }
}

/// What a repository nobody claims is called: its own name.
pub fn implicit_project_name(repo_full_name: &str) -> String {
    repo_full_name.rsplit('/').next().unwrap_or(repo_full_name).to_string()
}

impl KnowledgeConfig {
    /// The knowledge root, `~` expanded.
    pub fn root_path(&self) -> PathBuf {
        match &self.root {
            Some(root) => expand_home(root),
            None => home_dir().join(".perch").join("knowledge"),
        }
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
            Ok(mut c) => {
                if !c.knowledge.project.is_empty() {
                    c.warnings.push(
                        "config.toml: [[knowledge.project]] is no longer read; declare projects as [[project]]".into(),
                    );
                }
                c
            }
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
            agent: AgentConfig::default(),
            project: Vec::new(),
            warnings: Vec::new(),
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
    fn the_agent_section_defaults_and_overrides() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.agent.binary, "claude");
        assert_eq!(c.agent.model, "sonnet");
        assert_eq!(c.agent.models, vec!["fable", "opus", "sonnet", "haiku"]);
        let c: Config = toml::from_str("[agent]\nbinary = \"/opt/claude\"\neffort = \"high\"").unwrap();
        assert_eq!(c.agent.binary, "/opt/claude");
        assert_eq!(c.agent.effort, "high");
        assert_eq!(c.agent.model, "sonnet");
    }

    #[test]
    fn projects_map_repositories_and_fall_back_to_the_repository_name() {
        let c: Config = toml::from_str(
            r#"
            [knowledge]
            root = "~/elsewhere"
            skills = false

            [[project]]
            name = "slatedb"
            repos = ["slatedb/slatedb", "slatedb/slatedb-go"]
            "#,
        )
        .unwrap();
        assert_eq!(c.project_for("slatedb/slatedb-go"), "slatedb");
        assert_eq!(c.project_for("criccomini/perch"), "perch");
        assert!(!c.knowledge.skills);
        assert_eq!(c.knowledge.root_path(), home_dir().join("elsewhere"));
    }

    #[test]
    fn a_project_parses_with_and_without_roots() {
        let c: Config = toml::from_str(
            r#"
            [[project]]
            name = "perch"
            repos = ["criccomini/perch"]
            roots = ["~/Code/perch", "/tmp/perch-2"]

            [[project]]
            name = "slatedb"
            repos = ["slatedb/slatedb"]
            "#,
        )
        .unwrap();
        assert_eq!(c.project.len(), 2);
        assert_eq!(c.project[0].root_paths(), vec![home_dir().join("Code/perch"), PathBuf::from("/tmp/perch-2")]);
        assert!(c.project[1].roots.is_empty());
        assert!(c.project[1].root_paths().is_empty());
    }

    #[test]
    fn the_old_knowledge_project_key_still_parses_but_is_not_a_project() {
        let c: Config = toml::from_str("[[knowledge.project]]\nname = \"x\"\nrepos = [\"a/b\"]").unwrap();
        assert!(c.project.is_empty());
        assert_eq!(c.project_for("a/b"), "b", "the old key no longer maps");
    }
}
