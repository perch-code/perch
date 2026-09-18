//! Config file and on-disk paths.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

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
///
/// The top-level keys mean Claude Code, as they did before there was a
/// second backend, so an old file keeps working. Codex lives under
/// `[agent.codex]`. A model name picks the backend: one listed only under
/// Codex goes to Codex, anything else to Claude Code.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AgentConfig {
    /// The Claude Code executable. Found on `PATH` unless a path is given here.
    #[serde(default = "default_binary")]
    pub binary: String,
    /// What the picker starts on.
    #[serde(default = "default_model")]
    pub model: String,
    /// `low`, `medium`, `high`, `xhigh`, `max` or `ultra`.
    #[serde(default = "default_effort")]
    pub effort: String,
    /// The Claude Code models the picker offers.
    #[serde(default = "default_models")]
    pub models: Vec<String>,
    #[serde(default)]
    pub codex: CodexConfig,
}

/// The second backend. Absent from an old file is the usual case: the
/// binary is looked for, and the models come from Codex's own catalog.
/// Naming models here pins the list instead.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CodexConfig {
    /// The Codex executable. Found on `PATH` unless a path is given here.
    #[serde(default = "default_codex_binary")]
    pub binary: String,
    /// The Codex models the picker offers, after the Claude Code ones.
    /// Empty in the file means ask Codex; empty after that means Codex
    /// is not offered.
    #[serde(default)]
    pub models: Vec<String>,
    /// The effort names each model takes, from the catalog, keyed by
    /// model. Filled by discovery at startup, never by the file. A model
    /// not listed here is assumed to take every level.
    #[serde(skip)]
    pub efforts: BTreeMap<String, Vec<String>>,
}

/// The backend names, spelled as `perch-agent` spells them. Strings rather
/// than its enum so this crate does not have to depend on it.
pub const CLAUDE_CODE: &str = "claude-code";
pub const CODEX: &str = "codex";

fn default_binary() -> String {
    "claude".into()
}
fn default_codex_binary() -> String {
    "codex".into()
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
            codex: CodexConfig::default(),
        }
    }
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self { binary: default_codex_binary(), models: Vec::new(), efforts: BTreeMap::new() }
    }
}

impl AgentConfig {
    /// Which backend runs a model. Codex only when the name is listed
    /// under Codex and not under Claude Code, so a name in both, or one
    /// nobody lists, goes where it always went.
    pub fn backend_of(&self, model: &str) -> &'static str {
        let claude = self.models.iter().any(|m| m == model);
        let codex = self.codex.models.iter().any(|m| m == model);
        if codex && !claude { CODEX } else { CLAUDE_CODE }
    }

    /// The executable for a backend name. An unknown name gets Claude
    /// Code's, the same fallback `backend_of` makes.
    pub fn binary_for(&self, backend: &str) -> &str {
        if backend == CODEX { &self.codex.binary } else { &self.binary }
    }

    /// What the picker lists after the Claude Code models.
    pub fn codex_models(&self) -> &[String] {
        &self.codex.models
    }

    /// Whether a model takes an effort level. Only Codex's catalog says,
    /// and only for the models it listed; any other model is taken to
    /// take them all. Codex runs a level a model lacks at the nearest one
    /// it has, so this is what lets the picker say so rather than let it
    /// happen.
    pub fn effort_supported(&self, model: &str, effort: &str) -> bool {
        match self.codex.efforts.get(model) {
            Some(levels) => levels.iter().any(|l| l == effort),
            None => true,
        }
    }
}

/// Where archived discussion goes and which repositories share a project.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct KnowledgeConfig {
    /// Defaults to `knowledge/` under the Perch home. A leading `~` is the
    /// home directory.
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
    /// The knowledge root, `~` expanded: `knowledge/` under the Perch home
    /// unless the file says otherwise.
    pub fn root_path(&self) -> PathBuf {
        match &self.root {
            Some(root) => expand_home(root),
            None => perch_home().join("knowledge"),
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

/// Where everything Perch keeps lives: `$PERCH_HOME`, else `~/.perch`.
///
/// One directory, so it can be named in a sentence, backed up, or deleted
/// as a whole. Under it: `config.toml`, `perch.db`, `avatars/`, `repos/`,
/// `worktrees/`, `homes/`, and `knowledge/`.
pub fn perch_home() -> PathBuf {
    match std::env::var_os("PERCH_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => expand_home(&dir.to_string_lossy()),
        None => home_dir().join(".perch"),
    }
}

pub fn config_path() -> Option<PathBuf> {
    Some(perch_home().join("config.toml"))
}

/// The home, made if it is not there yet.
pub fn data_dir() -> Option<PathBuf> {
    let dir = perch_home();
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
    fn the_home_follows_the_variable_and_holds_everything() {
        // Set and unset in one test, since the variable is process-wide.
        std::env::set_var("PERCH_HOME", "/tmp/perch-test-home");
        assert_eq!(perch_home(), PathBuf::from("/tmp/perch-test-home"));
        assert_eq!(config_path(), Some(PathBuf::from("/tmp/perch-test-home/config.toml")));
        assert_eq!(KnowledgeConfig::default().root_path(), PathBuf::from("/tmp/perch-test-home/knowledge"));
        std::env::set_var("PERCH_HOME", "~/elsewhere");
        assert_eq!(perch_home(), home_dir().join("elsewhere"));
        std::env::remove_var("PERCH_HOME");
        assert_eq!(perch_home(), home_dir().join(".perch"));
    }

    #[test]
    fn an_old_config_still_parses() {
        let c: Config = toml::from_str("poll_interval_secs = 30").unwrap();
        assert_eq!(c.poll_interval_secs, 30);
        assert!(c.knowledge.skills);
        assert!(c.knowledge.project.is_empty());
        assert!(c.knowledge.root_path().ends_with("knowledge"));
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
    fn an_agent_table_without_codex_offers_no_codex_models() {
        let c: Config = toml::from_str("[agent]\nmodel = \"opus\"").unwrap();
        assert_eq!(c.agent.codex, CodexConfig::default());
        assert_eq!(c.agent.codex.binary, "codex");
        assert!(c.agent.codex_models().is_empty());
        assert_eq!(c.agent.backend_of("opus"), CLAUDE_CODE);
    }

    #[test]
    fn a_codex_table_parses_and_picks_the_backend_by_model() {
        let c: Config = toml::from_str(
            r#"
            [agent]
            effort = "ultra"

            [agent.codex]
            binary = "/opt/codex"
            models = ["gpt-5.6-sol"]
            "#,
        )
        .unwrap();
        assert_eq!(c.agent.effort, "ultra");
        assert_eq!(c.agent.codex.binary, "/opt/codex");
        assert_eq!(c.agent.codex_models(), ["gpt-5.6-sol".to_string()]);
        assert_eq!(c.agent.backend_of("gpt-5.6-sol"), CODEX);
        assert_eq!(c.agent.backend_of("sonnet"), CLAUDE_CODE);
        assert_eq!(c.agent.backend_of("nobody-lists-this"), CLAUDE_CODE);
        assert_eq!(c.agent.binary_for(CODEX), "/opt/codex");
        assert_eq!(c.agent.binary_for(CLAUDE_CODE), "claude");
        assert_eq!(c.agent.binary_for("something-else"), "claude");
    }

    #[test]
    fn the_catalog_says_which_efforts_a_model_takes_and_silence_means_all() {
        let mut c = AgentConfig::default();
        c.codex.efforts.insert("gpt-5.5".into(), vec!["low".into(), "high".into()]);
        assert!(c.effort_supported("gpt-5.5", "high"));
        assert!(!c.effort_supported("gpt-5.5", "ultra"));
        assert!(c.effort_supported("gpt-6-astra", "ultra"), "not in the catalog, so not doubted");
        assert!(c.effort_supported("sonnet", "ultra"), "Claude Code has no catalog");
        let parsed: Config = toml::from_str("[agent.codex]\nmodels = [\"x\"]").unwrap();
        assert!(parsed.agent.codex.efforts.is_empty(), "the file never fills it");
    }

    #[test]
    fn a_model_listed_under_both_backends_stays_with_claude() {
        let c: Config = toml::from_str("[agent]\nmodels = [\"shared\"]\n[agent.codex]\nmodels = [\"shared\"]").unwrap();
        assert_eq!(c.agent.backend_of("shared"), CLAUDE_CODE);
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
