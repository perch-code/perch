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
    /// `[[repo]]` entries: the repositories Perch lists, syncs, and files
    /// knowledge under.
    #[serde(default)]
    pub repo: Vec<RepoConfig>,
    /// The old `[[project]]` key. Read only so `load` can refuse the file
    /// by name: with no field for it, toml would drop the table without a
    /// word and the reader would see an empty sidebar and no reason.
    #[serde(default)]
    project: Vec<toml::Value>,
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

/// Where archived discussion goes.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct KnowledgeConfig {
    /// Defaults to `knowledge/` under the Perch home. A leading `~` is the
    /// home directory.
    #[serde(default)]
    pub root: Option<String>,
    /// Write the two `SKILL.md` files. On unless turned off.
    #[serde(default = "default_true")]
    pub skills: bool,
    /// The older home of `[[project]]`. Read only so `load` can refuse the
    /// file by name, as with `Config::project`.
    #[serde(default)]
    project: Vec<toml::Value>,
}

/// A repository the reader follows: `owner/repo`, and any checkouts of it
/// already on this machine.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct RepoConfig {
    /// `owner/repo`. The identity, compared without regard to case.
    pub name: String,
    /// Directories that are checkouts of the repository, or worktrees of
    /// one. A leading `~` is the home directory.
    #[serde(default)]
    pub roots: Vec<String>,
}

impl RepoConfig {
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
    /// Whether a repository is one of the `[[repo]]` entries. Declaring one
    /// is what starts the list pass and the backfill for it; an item in a
    /// repository nobody declared is still an item.
    pub fn is_declared(&self, repo_full_name: &str) -> bool {
        self.entry(repo_full_name).is_some()
    }

    /// The declared roots of a repository, `~` expanded. Empty when it is
    /// not declared, or declared without any.
    pub fn roots_for(&self, repo_full_name: &str) -> Vec<PathBuf> {
        self.entry(repo_full_name).map(RepoConfig::root_paths).unwrap_or_default()
    }

    /// The declared names in config order, which is the order the sidebar
    /// and the list pass take them in.
    pub fn declared(&self) -> Vec<String> {
        self.repo.iter().map(|r| r.name.clone()).collect()
    }

    fn entry(&self, repo_full_name: &str) -> Option<&RepoConfig> {
        self.repo.iter().find(|r| r.name.eq_ignore_ascii_case(repo_full_name))
    }
}

/// Whether a name is `owner/repo`: one slash, something on each side.
fn is_full_name(name: &str) -> bool {
    match name.split_once('/') {
        Some((owner, repo)) => !owner.is_empty() && !repo.is_empty() && !repo.contains('/'),
        None => false,
    }
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
        match Self::parse(&text) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "ignoring malformed config");
                Self::fallback()
            }
        }
    }

    /// The file's text as a config. A file that still declares its
    /// repositories under `[[project]]` or `[[knowledge.project]]` is not
    /// loaded at all: the fallback comes back with one warning naming the
    /// new key. Taking everything but that table would look like a config
    /// with no repositories, which is harder to notice than one with none
    /// of the reader's settings and a line saying why.
    fn parse(text: &str) -> Result<Self, toml::de::Error> {
        let mut c = toml::from_str::<Config>(text)?;
        let old_key = if !c.project.is_empty() {
            Some("[[project]]")
        } else if !c.knowledge.project.is_empty() {
            Some("[[knowledge.project]]")
        } else {
            None
        };
        if let Some(key) = old_key {
            let mut refused = Self::fallback();
            refused.warnings.push(format!(
                "config.toml: {key} is no longer read; declare each repository as [[repo]] with name = \"owner/repo\". The file was not loaded."
            ));
            return Ok(refused);
        }
        c.keep_well_formed_names();
        Ok(c)
    }

    /// Drop a `[[repo]]` whose name is not `owner/repo`, with a warning
    /// naming it. Everything downstream splits the name on its one slash,
    /// and a name that does not split would be a section nothing could
    /// list or file under. The rest of the file loads.
    fn keep_well_formed_names(&mut self) {
        let (kept, dropped): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.repo).into_iter().partition(|r| is_full_name(&r.name));
        for r in dropped {
            self.warnings.push(format!(
                "config.toml: [[repo]] name = {:?} is not owner/repo; the entry was skipped",
                r.name
            ));
        }
        self.repo = kept;
    }

    fn fallback() -> Self {
        Config {
            token: None,
            poll_interval_secs: default_poll_secs(),
            knowledge: KnowledgeConfig::default(),
            agent: AgentConfig::default(),
            repo: Vec::new(),
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
        assert!(c.repo.is_empty());
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
    fn a_declared_repository_is_known_by_its_whole_name_whatever_the_case() {
        let c = Config::parse(
            r#"
            [knowledge]
            root = "~/elsewhere"
            skills = false

            [[repo]]
            name = "slatedb/slatedb"

            [[repo]]
            name = "slatedb/slatedb-go"
            "#,
        )
        .unwrap();
        assert!(c.warnings.is_empty());
        assert_eq!(c.declared(), vec!["slatedb/slatedb", "slatedb/slatedb-go"]);
        assert!(c.is_declared("slatedb/slatedb-go"));
        assert!(c.is_declared("SlateDB/SlateDB"), "the name is an identity, not a spelling");
        assert!(!c.is_declared("slatedb"), "the whole owner/repo, not a part of it");
        assert!(!c.is_declared("criccomini/perch"));
        assert!(!c.knowledge.skills);
        assert_eq!(c.knowledge.root_path(), home_dir().join("elsewhere"));
    }

    #[test]
    fn a_repository_parses_with_and_without_roots() {
        let c = Config::parse(
            r#"
            [[repo]]
            name = "criccomini/perch"
            roots = ["~/Code/perch", "/tmp/perch-2"]

            [[repo]]
            name = "slatedb/slatedb"
            "#,
        )
        .unwrap();
        assert_eq!(c.repo.len(), 2);
        assert_eq!(c.repo[0].root_paths(), vec![home_dir().join("Code/perch"), PathBuf::from("/tmp/perch-2")]);
        assert_eq!(c.roots_for("Criccomini/Perch"), c.repo[0].root_paths());
        assert!(c.repo[1].roots.is_empty());
        assert!(c.roots_for("slatedb/slatedb").is_empty());
        assert!(c.roots_for("nobody/declared").is_empty());
    }

    #[test]
    fn the_old_project_key_refuses_the_file_and_names_the_new_one() {
        let c = Config::parse(
            r#"
            poll_interval_secs = 5

            [[project]]
            name = "slatedb"
            repos = ["slatedb/slatedb"]
            "#,
        )
        .unwrap();
        assert_eq!(c.warnings.len(), 1);
        assert!(c.warnings[0].starts_with("config.toml: [[project]] is no longer read"), "{}", c.warnings[0]);
        assert!(c.warnings[0].contains("[[repo]]"), "the message names the new key");
        assert!(c.repo.is_empty());
        assert_eq!(c.poll_interval_secs, 60, "nothing else in the file was taken either");
    }

    #[test]
    fn the_old_knowledge_project_key_refuses_the_file_too() {
        let c = Config::parse("[[knowledge.project]]\nname = \"x\"\nrepos = [\"a/b\"]").unwrap();
        assert_eq!(c.warnings.len(), 1);
        assert!(c.warnings[0].starts_with("config.toml: [[knowledge.project]] is no longer read"), "{}", c.warnings[0]);
        assert!(c.warnings[0].contains("[[repo]]"));
        assert!(c.repo.is_empty());
    }

    #[test]
    fn a_name_that_is_not_owner_and_repo_is_skipped_with_a_warning() {
        let c = Config::parse(
            r#"
            [[repo]]
            name = "slatedb"

            [[repo]]
            name = "slatedb/slatedb"

            [[repo]]
            name = "a/b/c"
            "#,
        )
        .unwrap();
        assert_eq!(c.declared(), vec!["slatedb/slatedb"], "the rest of the file loads");
        assert_eq!(c.warnings.len(), 2);
        assert!(c.warnings[0].contains("\"slatedb\""), "{}", c.warnings[0]);
        assert!(c.warnings[1].contains("\"a/b/c\""), "{}", c.warnings[1]);
    }

    #[test]
    fn a_malformed_file_is_an_error_not_a_config() {
        assert!(Config::parse("poll_interval_secs = = 3").is_err());
    }
}
