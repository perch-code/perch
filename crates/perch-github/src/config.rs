//! Config file and on-disk paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    pub agent: AgentConfig,
    /// A GitHub Enterprise host whose remotes count as GitHub, beside
    /// github.com.
    #[serde(default)]
    pub github_host: Option<String>,
    /// `[[repo]]` entries: the repositories Perch knows about.
    #[serde(default)]
    pub repo: Vec<RepoConfig>,
    /// `[work]`: how new work is set up in every repository.
    #[serde(default)]
    pub work: WorkDefaults,
    /// `[lsp]`: the language servers Perch runs.
    #[serde(default)]
    pub lsp: LspConfig,
    /// `[display]`: how big Perch draws. Read at startup.
    #[serde(default)]
    pub display: DisplayConfig,
    /// `[ui]`: how the window looks.
    #[serde(default)]
    pub ui: UiConfig,
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
    /// Filled by startup discovery and copied into durable run provenance.
    #[serde(skip)]
    pub binary_version: Option<String>,
    /// What the picker starts on.
    #[serde(default = "default_model")]
    pub model: String,
    /// `low`, `medium`, `high`, `xhigh`, `max` or `ultra`.
    #[serde(default = "default_effort")]
    pub effort: String,
    /// The Claude Code models the picker offers.
    #[serde(default = "default_models")]
    pub models: Vec<String>,
    /// Maximum top-level agent processes. Additional work waits in the
    /// durable FIFO queue; native subagents remain part of their parent.
    #[serde(default = "default_max_concurrent_runs")]
    pub max_concurrent_runs: usize,
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
    /// Filled by startup discovery and copied into durable run provenance.
    #[serde(skip)]
    pub binary_version: Option<String>,
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

/// What the picker starts on when Claude Code is missing and Codex offers
/// it: GPT-6.1-Sol.
pub const CODEX_DEFAULT_MODEL: &str = "gpt-6.1-sol";

fn default_binary() -> String {
    "claude".into()
}
fn default_codex_binary() -> String {
    "codex".into()
}
fn default_model() -> String {
    "opus".into()
}
fn default_effort() -> String {
    "high".into()
}
fn default_models() -> Vec<String> {
    // The aliases the CLI resolves to the latest of each tier.
    vec![
        "fable".into(),
        "opus".into(),
        "sonnet".into(),
        "haiku".into(),
    ]
}
fn default_max_concurrent_runs() -> usize {
    4
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            binary: default_binary(),
            binary_version: None,
            model: default_model(),
            effort: default_effort(),
            models: default_models(),
            max_concurrent_runs: default_max_concurrent_runs(),
            codex: CodexConfig::default(),
        }
    }
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            binary: default_codex_binary(),
            binary_version: None,
            models: Vec::new(),
            efforts: BTreeMap::new(),
        }
    }
}

impl AgentConfig {
    /// Which backend runs a model. Codex only when the name is listed
    /// under Codex and not under Claude Code, so a name in both, or one
    /// nobody lists, goes where it always went.
    pub fn backend_of(&self, model: &str) -> &'static str {
        let claude = self.models.iter().any(|m| m == model);
        let codex = self.codex.models.iter().any(|m| m == model);
        if codex && !claude {
            CODEX
        } else {
            CLAUDE_CODE
        }
    }

    /// The executable for a backend name. An unknown name gets Claude
    /// Code's, the same fallback `backend_of` makes.
    pub fn binary_for(&self, backend: &str) -> &str {
        if backend == CODEX {
            &self.codex.binary
        } else {
            &self.binary
        }
    }

    pub fn binary_version_for(&self, backend: &str) -> Option<&str> {
        if backend == CODEX {
            self.codex.binary_version.as_deref()
        } else {
            self.binary_version.as_deref()
        }
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

/// The owner reserved for repositories with no GitHub remote. Their
/// identity is `local/<name>`, so it splits on one slash and files under
/// two directories like any other; `parse` refuses it in `github`, so a
/// GitHub repository cannot collide with one.
pub const LOCAL_OWNER: &str = "local";

/// Whether an identity is `local/<name>`: a repository with no GitHub
/// remote, which no GitHub subject lookup should ask about. The owner is compared
/// without regard to case, as identities are everywhere else.
pub fn is_local(identity: &str) -> bool {
    match identity.split_once('/') {
        Some((owner, _)) => owner.eq_ignore_ascii_case(LOCAL_OWNER),
        None => false,
    }
}

/// `[ui]`: how the window looks.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct UiConfig {
    /// The family code is set in, by its installed name. Off macOS only:
    /// there it is SF Mono, as the rest of the system's code is.
    #[serde(default)]
    pub code_font: Option<String>,
}

/// `[lsp]`: the language servers Perch runs.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LspConfig {
    /// What every language server Perch runs may use between them, in
    /// megabytes, before the ones kept for a return to an earlier item are
    /// stopped, oldest first. The ones on show are never stopped, so the
    /// total can stay above this.
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: u64,
}

fn default_max_memory_mb() -> u64 {
    8 * 1024
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            max_memory_mb: default_max_memory_mb(),
        }
    }
}

/// `[display]`: how big Perch draws, as an editor sets a window zoom and
/// a code font size apart.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DisplayConfig {
    /// Everything in the window, text, code and spacing, times this.
    #[serde(default = "default_zoom")]
    pub zoom: f64,
    /// The code font, in points before `zoom`. The UI's own text keeps
    /// its sizes; only `zoom` moves those.
    #[serde(default = "default_code_font_size")]
    pub code_font_size: f32,
}

pub const ZOOM_RANGE: std::ops::RangeInclusive<f64> = 0.75..=2.0;
pub const CODE_FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 8.0..=32.0;

fn default_zoom() -> f64 {
    1.0
}
fn default_code_font_size() -> f32 {
    12.0
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            zoom: default_zoom(),
            code_font_size: default_code_font_size(),
        }
    }
}

impl DisplayConfig {
    /// Hold each setting to what draws: a zoom in range, and a code size
    /// in range and whole, since the editor lays code out at a whole
    /// size. Says what it changed, one line each.
    fn keep_in_range(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        let zoom = if self.zoom.is_finite() {
            self.zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end())
        } else {
            default_zoom()
        };
        if zoom != self.zoom {
            warnings.push(format!(
                "config.toml: [display] zoom = {} was read as {zoom}; it goes from {} to {}",
                self.zoom,
                ZOOM_RANGE.start(),
                ZOOM_RANGE.end()
            ));
            self.zoom = zoom;
        }
        let size = if self.code_font_size.is_finite() {
            self.code_font_size
                .round()
                .clamp(*CODE_FONT_SIZE_RANGE.start(), *CODE_FONT_SIZE_RANGE.end())
        } else {
            default_code_font_size()
        };
        if size != self.code_font_size {
            warnings.push(format!(
                "config.toml: [display] code_font_size = {} was read as {size}; it is a whole size from {} to {}",
                self.code_font_size,
                CODE_FONT_SIZE_RANGE.start(),
                CODE_FONT_SIZE_RANGE.end()
            ));
            self.code_font_size = size;
        }
        warnings
    }
}

/// `[work]`: settings for new work in any repository.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct WorkDefaults {
    /// Put before a branch name the new-work view fills in, such as
    /// `chris/`. A name the reader types is taken as typed.
    #[serde(default)]
    pub branch_prefix: String,
    /// Who decides what a Work run's sandbox refuses. `never` is taken
    /// only from `[repo.work]`, one repository at a time.
    #[serde(default)]
    pub approvals: Approvals,
}

/// Who decides a Work run's requests to go past its sandbox.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Approvals {
    /// The CLI's own automatic reviewer: Claude Code's auto permission
    /// mode, Codex's `auto_review`. What it passes on comes to the reader.
    #[default]
    Auto,
    /// Every request comes to the reader.
    Ask,
    /// Perch allows every request without asking. The sandbox still
    /// holds what the run does without a request, and the protected paths
    /// stay denied.
    Never,
}

/// `[repo.work]`: what a Work run may reach outside its worktree, in one
/// repository. Both lists are empty unless the file fills them.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct RepoWork {
    /// Directories the run may write, such as a package manager's cache.
    /// A leading `~` is the home directory.
    #[serde(default)]
    pub write: Vec<String>,
    /// Domains the run may reach, such as `index.crates.io`.
    #[serde(default)]
    pub network: Vec<String>,
    /// Load the repository's own agent configuration for Work runs: its
    /// `.claude/` settings, `.mcp.json` and `.codex/`. What is there runs
    /// as the reader, outside any sandbox.
    #[serde(default)]
    pub trust: bool,
    /// Overrides `[work] approvals` for this repository.
    #[serde(default)]
    pub approvals: Option<Approvals>,
}

impl RepoWork {
    pub fn write_paths(&self) -> Vec<PathBuf> {
        self.write.iter().map(|w| expand_home(w)).collect()
    }
}

/// `[repo.lsp]`: the language servers in one repository.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct RepoLsp {
    /// Start rust-analyzer and jdtls on every item here without asking,
    /// anyone's pull request included. They run the checkout's build
    /// scripts, proc macros and Gradle or Maven build as the reader.
    #[serde(default)]
    pub trust: bool,
}

/// Why a `[repo.work] write` entry is refused, if it is: it is, or holds,
/// the home directory or Perch's own, or it is in or holds a root's git
/// directory. A run that could write any of those could change what runs
/// outside its sandbox: a shell profile, Perch's database, a hook.
fn refused_write(path: &Path, roots: &[PathBuf]) -> Option<String> {
    let path = normalize(path);
    let covers = |other: &Path| other.starts_with(&path);
    let home = home_dir();
    if covers(&home) {
        return Some("it is or holds the home directory".into());
    }
    let perch = perch_home();
    if covers(&perch) || path.starts_with(&perch) {
        return Some(format!("it is or is in {}", perch.display()));
    }
    for root in roots {
        let git = root.join(".git");
        if covers(&git) || path.starts_with(&git) {
            return Some(format!("it is or is in {}", git.display()));
        }
    }
    None
}

/// `.` and `..` taken out of a path without asking the disk, so
/// `~/Code/../` reads as the home directory it is.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Add a `[[repo]]` with one root to the end of the config file, making
/// the file when there is none. The file's text is appended to, never
/// rewritten, so its comments and order stay as the reader left them. A
/// root under the home directory is written with `~`.
pub fn append_repo(config: &Path, root: &Path) -> std::io::Result<()> {
    use std::io::Write;

    let home = home_dir();
    let shown = match root.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => root.display().to_string(),
    };
    let roots = toml::Value::Array(vec![toml::Value::String(shown)]);
    let existing = match std::fs::read_to_string(config) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut add = String::new();
    if !existing.is_empty() {
        if !existing.ends_with('\n') {
            add.push('\n');
        }
        add.push('\n');
    }
    add.push_str(&format!("[[repo]]\nroots = {roots}\n"));
    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config)?
        .write_all(add.as_bytes())
}

/// Take out the `[[repo]]` that reads as `entry`, with the `[repo.*]`
/// tables under it and the comment lines just above it. The rest of the
/// file's text stays as the reader left it. False when no table in the
/// file reads as `entry`. The file is replaced whole, keeping its
/// permissions, and only once what is left still loads with one
/// repository fewer.
pub fn remove_repo(config: &Path, entry: &RepoConfig) -> std::io::Result<bool> {
    let text = std::fs::read_to_string(config)?;
    let Some(cut) = repo_table(&text, entry) else {
        return Ok(false);
    };
    let mut left = format!("{}{}", &text[..cut.start], &text[cut.end..]);
    if cut.end == text.len() {
        left.truncate(left.trim_end().len());
        if !left.is_empty() {
            left.push('\n');
        }
    }
    let had = Config::parse(&text).map(|c| c.repo.len()).unwrap_or(0);
    match Config::parse(&left) {
        Ok(c) if c.repo.len() + 1 == had => {}
        _ => {
            return Err(std::io::Error::other(
                "the file would not have read as one repository fewer, so it was left alone",
            ))
        }
    }
    let temp = config.with_extension("toml.removing");
    std::fs::write(&temp, &left)?;
    std::fs::set_permissions(&temp, std::fs::metadata(config)?.permissions())?;
    std::fs::rename(&temp, config)?;
    Ok(true)
}

/// Where the first `[[repo]]` that reads as `entry` sits in the file's
/// text: from the comments over its header to the comments over the next
/// table that is not one of its own `[repo.*]`.
fn repo_table(text: &str, entry: &RepoConfig) -> Option<std::ops::Range<usize>> {
    struct Line {
        start: usize,
        /// A header's table name, and whether it is `[[..]]`.
        header: Option<(String, bool)>,
        comment: bool,
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        let name = |s: &str| s.replace(char::is_whitespace, "");
        let header = if let Some(rest) = trimmed.strip_prefix("[[") {
            rest.split_once("]]").map(|(n, _)| (name(n), true))
        } else if let Some(rest) = trimmed.strip_prefix('[') {
            rest.split_once(']').map(|(n, _)| (name(n), false))
        } else {
            None
        };
        lines.push(Line {
            start: at,
            header,
            comment: trimmed.starts_with('#'),
        });
        at += line.len();
    }
    // Where the comment lines with no gap above line `i` start.
    let comments_over = |i: usize| {
        let mut top = i;
        while top > 0 && lines[top - 1].comment {
            top -= 1;
        }
        lines[top].start
    };
    let is_repo =
        |header: &Option<(String, bool)>| matches!(header, Some((name, true)) if name == "repo");
    for i in 0..lines.len() {
        if !is_repo(&lines[i].header) {
            continue;
        }
        // The next table that is not one of this entry's `[repo.*]`.
        let next = (i + 1..lines.len()).find(|&j| match &lines[j].header {
            Some((name, _)) => is_repo(&lines[j].header) || !name.starts_with("repo."),
            None => false,
        });
        let start = comments_over(i);
        let end = next.map_or(text.len(), comments_over);
        let block = &text[lines[i].start..end];
        let reads_as = Config::parse(block).is_ok_and(|c| match c.repo.as_slice() {
            [r] => {
                r.github.as_deref().map(str::to_lowercase)
                    == entry.github.as_deref().map(str::to_lowercase)
                    && r.roots == entry.roots
                    && r.name == entry.name
            }
            _ => false,
        });
        if reads_as {
            return Some(start..end);
        }
    }
    None
}

/// A repository the reader follows. Its identity is `owner/repo` on
/// GitHub, or `local/<name>` for one that lives only on this machine;
/// either way it is compared without regard to case, and everything
/// downstream splits it on its one slash. `name` is what the sidebar
/// shows and nothing more.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct RepoConfig {
    /// `owner/repo` on GitHub. Absent for a repository that lives only on
    /// this machine.
    #[serde(default)]
    pub github: Option<String>,
    /// What the sidebar calls it. Defaults to the repo half of `github`,
    /// else the first root's basename.
    #[serde(default)]
    pub name: Option<String>,
    /// Directories that are checkouts of the repository, or worktrees of
    /// one. A leading `~` is the home directory.
    #[serde(default)]
    pub roots: Vec<String>,
    /// The base branch, used when no remote HEAD says which it is.
    #[serde(default)]
    pub base: Option<String>,
    /// `[repo.work]`: what a Work run in this repository may reach beyond
    /// its worktree.
    #[serde(default)]
    pub work: RepoWork,
    /// `[repo.lsp]`: whether servers that run a checkout's code start here
    /// without asking.
    #[serde(default)]
    pub lsp: RepoLsp,
    /// The identity once known: `github` when given, else set by `resolve`
    /// from the first root's scan.
    #[serde(skip)]
    identity: Option<String>,
}

impl RepoConfig {
    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|r| expand_home(r)).collect()
    }

    /// The identity, once there is one: `github` when the file gave it,
    /// else what `resolve` settled on. None for a roots-only entry the
    /// scan has not reached yet.
    pub fn identity(&self) -> Option<&str> {
        self.github.as_deref().or(self.identity.as_deref())
    }

    pub fn is_resolved(&self) -> bool {
        self.identity().is_some()
    }

    /// What the sidebar shows: `name`, else the repo half of `github`,
    /// else the first root's basename. Empty only for an entry `parse`
    /// would not have kept.
    pub fn display_name(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        if let Some((_, repo)) = self.github.as_deref().and_then(|g| g.split_once('/')) {
            return repo.to_string();
        }
        self.root_paths()
            .first()
            .and_then(|root| root.file_name())
            .map(|base| base.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Give a roots-only entry its identity: `Some(slug)` from a GitHub
    /// remote makes it that repository, `None` makes it `local/<name>`.
    pub fn resolve(&mut self, github: Option<String>) {
        self.identity = Some(match github {
            Some(slug) => slug,
            None => format!("{LOCAL_OWNER}/{}", self.display_name()),
        });
    }

    /// How a warning names the entry: by its `github`, else its roots,
    /// else its `name`, else as the bare table.
    fn label(&self) -> String {
        if let Some(github) = &self.github {
            format!("[[repo]] github = {github:?}")
        } else if !self.roots.is_empty() {
            format!("[[repo]] roots = {:?}", self.roots)
        } else if let Some(name) = &self.name {
            format!("[[repo]] name = {name:?}")
        } else {
            "[[repo]]".to_string()
        }
    }
}

/// One declared repository as the sidebar needs it: the identity its items
/// carry, and the name its section shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredRepo {
    pub identity: String,
    pub name: String,
}

impl Config {
    /// The hosts whose remotes name a GitHub repository: github.com, and
    /// the enterprise host when the file names one.
    pub fn github_hosts(&self) -> Vec<String> {
        let mut hosts = vec!["github.com".to_string()];
        if let Some(host) = self.github_host.as_deref().filter(|h| !h.is_empty()) {
            hosts.push(host.to_string());
        }
        hosts
    }

    /// Whether an identity is one of the `[[repo]]` entries, `owner/repo`
    /// or `local/<name>`. Declaring one is what makes the sidebar a section
    /// for it. An item in a repository nobody declared is still an item.
    pub fn is_declared(&self, identity: &str) -> bool {
        self.entry(identity).is_some()
    }

    /// Who decides a Work run's requests in the repository with an
    /// identity: its `[repo.work] approvals`, else `[work] approvals`.
    pub fn approvals_for(&self, identity: &str) -> Approvals {
        self.entry(identity)
            .and_then(|r| r.work.approvals)
            .unwrap_or(self.work.approvals)
    }

    /// Whether `[repo.lsp] trust` is set for the repository with an identity.
    pub fn lsp_trusted(&self, identity: &str) -> bool {
        self.entry(identity).is_some_and(|r| r.lsp.trust)
    }

    /// The declared roots of the repository with an identity, `~` expanded.
    /// Empty when nothing declares it, or the entry has none.
    pub fn roots_for(&self, identity: &str) -> Vec<PathBuf> {
        self.entry(identity)
            .map(RepoConfig::root_paths)
            .unwrap_or_default()
    }

    /// The identities in config order, which is the order the sidebar uses.
    /// A roots-only entry is left out until `resolve` has given it one.
    pub fn declared(&self) -> Vec<String> {
        self.repo
            .iter()
            .filter_map(|r| r.identity().map(str::to_string))
            .collect()
    }

    /// The resolved entries as the sidebar needs them, in config order.
    pub fn declared_repos(&self) -> Vec<DeclaredRepo> {
        self.repo
            .iter()
            .filter_map(|r| {
                r.identity().map(|id| DeclaredRepo {
                    identity: id.to_string(),
                    name: r.display_name(),
                })
            })
            .collect()
    }

    /// What the sidebar calls an identity: its entry's name, or the
    /// identity itself when nothing declares it.
    pub fn display_name(&self, identity: &str) -> String {
        match self.entry(identity) {
            Some(r) => r.display_name(),
            None => identity.to_string(),
        }
    }

    /// After every entry is resolved: a later entry whose identity repeats
    /// an earlier one's is dropped with a warning naming both. `parse`
    /// already refused a repeated `github`, so what lands here is a
    /// roots-only entry whose checkout turned out to be of a repository
    /// declared above it.
    pub fn drop_repeated_identities(&mut self) {
        let mut kept: Vec<RepoConfig> = Vec::new();
        for r in std::mem::take(&mut self.repo) {
            let repeated = r.identity().and_then(|id| {
                let first = kept
                    .iter()
                    .find(|k| k.identity().is_some_and(|kid| kid.eq_ignore_ascii_case(id)))?;
                Some((id, first))
            });
            match repeated {
                Some((id, first)) => self.warnings.push(format!(
                    "config.toml: {} is {id}, which {} already declares; the later entry was skipped",
                    r.label(),
                    first.label()
                )),
                None => kept.push(r),
            }
        }
        self.repo = kept;
    }

    fn entry(&self, identity: &str) -> Option<&RepoConfig> {
        self.repo.iter().find(|r| {
            r.identity()
                .is_some_and(|id| id.eq_ignore_ascii_case(identity))
        })
    }
}

/// Whether a name is `owner/repo`: one slash, something on each side.
fn is_full_name(name: &str) -> bool {
    match name.split_once('/') {
        Some((owner, repo)) => !owner.is_empty() && !repo.is_empty() && !repo.contains('/'),
        None => false,
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
        Self::load_from(&path)
    }

    /// The config in `path`, or the defaults when it cannot be read.
    pub fn load_from(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
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
    /// repositories under `[[project]]` is not loaded at all: the fallback
    /// comes back with one warning naming the new key. Taking everything but
    /// that table would look like a config with no repositories, which is
    /// harder to notice than one with none of the reader's settings and a
    /// line saying why.
    fn parse(text: &str) -> Result<Self, toml::de::Error> {
        let mut c = toml::from_str::<Config>(text)?;
        if !c.project.is_empty() {
            let mut refused = Self::fallback();
            refused.warnings.push(
                "config.toml: [[project]] is no longer read; declare each repository as [[repo]] with github = \"owner/repo\". The file was not loaded."
                    .to_string(),
            );
            return Ok(refused);
        }
        c.keep_well_formed_repos();
        let display = c.display.keep_in_range();
        c.warnings.extend(display);
        if c.work.approvals == Approvals::Never {
            c.work.approvals = Approvals::Auto;
            c.warnings.push(
                "config.toml: [work] approvals = \"never\" was read as \"auto\"; set it under a repository's [repo.work] instead"
                    .to_string(),
            );
        }
        Ok(c)
    }

    /// Drop a `[[repo]]` that could not be identified or scanned, with a
    /// warning naming it, and let the rest of the file load. A `github`
    /// that is not `owner/repo` would not split the way everything
    /// downstream splits an identity; the owner `local` is kept for
    /// repositories with no GitHub remote; an entry with neither `github`
    /// nor roots has nothing to identify and nothing to scan; and two entries
    /// with the same `github`, or two the sidebar would call the same
    /// thing, are one repository or an ambiguity, so the later one goes.
    ///
    /// A file from before `github` existed said `name = "owner/repo"`.
    /// That reads as `github` when nothing else set it, with a notice, so
    /// nothing breaks on upgrade.
    ///
    /// An entry with `github` has its identity from here on. One with only
    /// roots waits for the scan to say whether its checkout is on GitHub.
    fn keep_well_formed_repos(&mut self) {
        let mut kept: Vec<RepoConfig> = Vec::new();
        for mut r in std::mem::take(&mut self.repo) {
            if r.github.is_none() && r.name.as_deref().is_some_and(|n| n.contains('/')) {
                let old = r.name.take().unwrap_or_default();
                self.warnings.push(format!(
                    "config.toml: [[repo]] name = {old:?} was read as github = {old:?}; the key is now github, and name is what the sidebar shows"
                ));
                r.github = Some(old);
            }
            if let Some(github) = r.github.as_deref().filter(|g| !is_full_name(g)) {
                self.warnings.push(format!(
                    "config.toml: [[repo]] github = {github:?} is not owner/repo; the entry was skipped"
                ));
                continue;
            }
            if r.github.as_deref().is_some_and(is_local) {
                self.warnings.push(format!(
                    "config.toml: {} has the owner {LOCAL_OWNER}, which is reserved for repositories with no GitHub remote; the entry was skipped",
                    r.label()
                ));
                continue;
            }
            if r.github.is_none() && r.roots.is_empty() {
                self.warnings.push(format!(
                    "config.toml: {} has neither github nor roots, so there is nothing to identify or scan; the entry was skipped",
                    r.label()
                ));
                continue;
            }
            if let Some(first) = kept.iter().find(|k| {
                k.github
                    .as_deref()
                    .zip(r.github.as_deref())
                    .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
            }) {
                self.warnings.push(format!(
                    "config.toml: {} repeats {}; the later entry was skipped",
                    r.label(),
                    first.label()
                ));
                continue;
            }
            let name = r.display_name();
            if let Some(first) = kept
                .iter()
                .find(|k| k.display_name().eq_ignore_ascii_case(&name))
            {
                self.warnings.push(format!(
                    "config.toml: {} and {} would both show as {name:?}; give one of them a name. The later entry was skipped",
                    r.label(),
                    first.label()
                ));
                continue;
            }
            let roots = r.root_paths();
            let label = r.label();
            let warnings = &mut self.warnings;
            r.work
                .write
                .retain(|entry| match refused_write(&expand_home(entry), &roots) {
                    Some(why) => {
                        warnings.push(format!(
                        "config.toml: {label} [repo.work] write = {entry:?} was left out: {why}"
                    ));
                        false
                    }
                    None => true,
                });
            kept.push(r);
        }
        self.repo = kept;
    }

    fn fallback() -> Self {
        Config {
            token: None,
            poll_interval_secs: default_poll_secs(),
            agent: AgentConfig::default(),
            github_host: None,
            repo: Vec::new(),
            work: WorkDefaults::default(),
            lsp: LspConfig::default(),
            display: DisplayConfig::default(),
            ui: UiConfig::default(),
            project: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// Where everything Perch keeps lives: `$PERCH_HOME`, else `~/.perch`.
///
/// One directory, so it can be named in a sentence, backed up, or deleted
/// as a whole. Under it: `config.toml`, `perch.db`, `avatars/`, `repos/`,
/// `worktrees/`, `homes/`, and `updates/`.
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
    fn a_repository_is_appended_and_the_file_before_it_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch").join("config.toml");

        append_repo(&path, Path::new("/srv/notes")).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        assert_eq!(first, "[[repo]]\nroots = [\"/srv/notes\"]\n");

        let mine = "# mine\npoll_interval_secs = 30\n\n[agent]\nmodel = \"opus\"";
        std::fs::write(&path, mine).unwrap();
        append_repo(&path, &home_dir().join("Code").join("wid\"gets")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(mine), "{text}");
        assert!(
            text.ends_with("\n\n[[repo]]\nroots = ['~/Code/wid\"gets']\n"),
            "{text}"
        );
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.poll_interval_secs, 30);
        assert_eq!(config.agent.model, "opus");
        assert_eq!(config.repo.len(), 1);
        assert_eq!(
            config.repo[0].root_paths(),
            [home_dir().join("Code").join("wid\"gets")]
        );
    }

    #[test]
    fn a_repository_is_taken_out_with_its_tables_and_comments_and_the_rest_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "# mine\npoll_interval_secs = 30\n\n[[repo]]\ngithub = \"a/one\"\n\n# the notes\n[[repo]]\nroots = [\"/srv/notes\"]\n[repo.work]\napprovals = \"ask\"\n\n# about two\n[[repo]]\ngithub = \"a/two\"\n\n[agent]\nmodel = \"opus\"\n";
        std::fs::write(&path, text).unwrap();
        let notes = Config::parse(text).unwrap().repo[1].clone();

        assert!(remove_repo(&path, &notes).unwrap());
        let left = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            left,
            "# mine\npoll_interval_secs = 30\n\n[[repo]]\ngithub = \"a/one\"\n\n# about two\n[[repo]]\ngithub = \"a/two\"\n\n[agent]\nmodel = \"opus\"\n"
        );
        let config = Config::parse(&left).unwrap();
        assert_eq!(config.declared(), ["a/one", "a/two"]);
        assert_eq!(config.agent.model, "opus");

        // Gone already: nothing reads as it, and the file is left alone.
        assert!(!remove_repo(&path, &notes).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), left);
    }

    #[test]
    fn the_last_repository_goes_without_leaving_blank_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[repo]]\ngithub = \"a/one\"\n\n[[repo]]\nname = \"a/two\"\n\n",
        )
        .unwrap();
        // A `name = "owner/repo"` from before `github` reads as the github.
        let two = Config::parse(&std::fs::read_to_string(&path).unwrap())
            .unwrap()
            .repo[1]
            .clone();
        assert_eq!(two.github.as_deref(), Some("a/two"));

        assert!(remove_repo(&path, &two).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[[repo]]\ngithub = \"a/one\"\n"
        );
        let one = Config::parse("[[repo]]\ngithub = \"A/One\"\n")
            .unwrap()
            .repo[0]
            .clone();
        assert!(remove_repo(&path, &one).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn work_approvals_are_automatic_unless_the_file_says_ask() {
        assert_eq!(Config::parse("").unwrap().work.approvals, Approvals::Auto);
        let ask = Config::parse("[work]\napprovals = \"ask\"\n").unwrap();
        assert_eq!(ask.work.approvals, Approvals::Ask);
    }

    #[test]
    fn never_asking_is_per_repository() {
        let config = Config::parse(
            "[work]\napprovals = \"ask\"\n\n[[repo]]\ngithub = \"a/one\"\n[repo.work]\napprovals = \"never\"\n\n[[repo]]\ngithub = \"a/two\"\n",
        )
        .unwrap();
        assert_eq!(config.approvals_for("a/one"), Approvals::Never);
        assert_eq!(config.approvals_for("A/One"), Approvals::Never);
        assert_eq!(config.approvals_for("a/two"), Approvals::Ask);
        assert_eq!(config.approvals_for("a/undeclared"), Approvals::Ask);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);

        let global = Config::parse("[work]\napprovals = \"never\"\n").unwrap();
        assert_eq!(global.work.approvals, Approvals::Auto);
        assert_eq!(global.warnings.len(), 1, "{:?}", global.warnings);
    }

    #[test]
    fn language_server_trust_is_per_repository() {
        let config = Config::parse(
            "[[repo]]\ngithub = \"a/one\"\n[repo.lsp]\ntrust = true\n\n[[repo]]\ngithub = \"a/two\"\n[repo.work]\ntrust = true\n",
        )
        .unwrap();
        assert!(config.lsp_trusted("a/one"));
        assert!(config.lsp_trusted("A/One"));
        assert!(
            !config.lsp_trusted("a/two"),
            "[repo.work] trust is for agents"
        );
        assert!(!config.lsp_trusted("a/undeclared"));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    }

    #[test]
    fn work_settings_are_read_and_a_dangerous_write_left_out() {
        let text = r#"
[work]
branch_prefix = "chris/"

[[repo]]
roots = ["/srv/notes"]

[repo.work]
write = ["~/.cargo/registry", "~", "/", "/srv/notes/.git/hooks", "/srv/notes/.git/..", "~/Code/../.perch"]
network = ["index.crates.io"]
"#;
        let config = Config::parse(text).unwrap();
        assert_eq!(config.work.branch_prefix, "chris/");
        let work = &config.repo[0].work;
        assert_eq!(work.write, ["~/.cargo/registry"]);
        assert_eq!(work.write_paths(), [home_dir().join(".cargo/registry")]);
        assert_eq!(work.network, ["index.crates.io"]);
        assert_eq!(config.warnings.len(), 5, "{:?}", config.warnings);
        assert!(
            config.warnings[0]
                .contains("write = \"~\" was left out: it is or holds the home directory"),
            "{:?}",
            config.warnings
        );
        assert!(
            config.warnings[2].contains("/srv/notes/.git"),
            "{:?}",
            config.warnings
        );
    }

    #[test]
    fn the_home_follows_the_variable_and_holds_everything() {
        // Set and unset in one test, since the variable is process-wide.
        std::env::set_var("PERCH_HOME", "/tmp/perch-test-home");
        assert_eq!(perch_home(), PathBuf::from("/tmp/perch-test-home"));
        assert_eq!(
            config_path(),
            Some(PathBuf::from("/tmp/perch-test-home/config.toml"))
        );
        std::env::set_var("PERCH_HOME", "~/elsewhere");
        assert_eq!(perch_home(), home_dir().join("elsewhere"));
        std::env::remove_var("PERCH_HOME");
        assert_eq!(perch_home(), home_dir().join(".perch"));
    }

    #[test]
    fn an_old_config_still_parses() {
        let c: Config = toml::from_str("poll_interval_secs = 30").unwrap();
        assert_eq!(c.poll_interval_secs, 30);
        assert!(c.repo.is_empty());
    }

    #[test]
    fn the_agent_section_defaults_and_overrides() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.agent.binary, "claude");
        assert_eq!(c.agent.model, "opus");
        assert_eq!(c.agent.effort, "high");
        assert_eq!(c.agent.models, vec!["fable", "opus", "sonnet", "haiku"]);
        assert_eq!(c.agent.max_concurrent_runs, 4);
        let c: Config = toml::from_str(
            "[agent]\nbinary = \"/opt/claude\"\neffort = \"low\"\nmax_concurrent_runs = 7",
        )
        .unwrap();
        assert_eq!(c.agent.binary, "/opt/claude");
        assert_eq!(c.agent.effort, "low");
        assert_eq!(c.agent.model, "opus");
        assert_eq!(c.agent.max_concurrent_runs, 7);
    }

    #[test]
    fn the_code_font_is_unset_unless_named() {
        assert_eq!(Config::fallback().ui.code_font, None);
        let c: Config = toml::from_str("[ui]\ncode_font = \"Fira Code\"").unwrap();
        assert_eq!(c.ui.code_font.as_deref(), Some("Fira Code"));
    }

    #[test]
    fn the_lsp_memory_budget_defaults_and_overrides() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.lsp.max_memory_mb, 8192);
        assert_eq!(Config::fallback().lsp.max_memory_mb, 8192);
        let c: Config = toml::from_str("[lsp]\nmax_memory_mb = 4096").unwrap();
        assert_eq!(c.lsp.max_memory_mb, 4096);
    }

    #[test]
    fn display_defaults_and_overrides() {
        let c = Config::parse("").unwrap();
        assert_eq!(c.display, DisplayConfig::default());
        assert_eq!((c.display.zoom, c.display.code_font_size), (1.0, 12.0));
        let c = Config::parse("[display]\nzoom = 1.25\ncode_font_size = 14").unwrap();
        assert_eq!((c.display.zoom, c.display.code_font_size), (1.25, 14.0));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn display_settings_out_of_range_are_held_to_it_with_a_warning() {
        let c = Config::parse("[display]\nzoom = 5.0\ncode_font_size = 13.4").unwrap();
        assert_eq!((c.display.zoom, c.display.code_font_size), (2.0, 13.0));
        assert_eq!(c.warnings.len(), 2, "{:?}", c.warnings);
        assert!(
            c.warnings[0].contains("zoom = 5 was read as 2"),
            "{}",
            c.warnings[0]
        );
        let c = Config::parse("[display]\ncode_font_size = 4").unwrap();
        assert_eq!(c.display.code_font_size, 8.0);
        assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
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
        c.codex
            .efforts
            .insert("gpt-5.5".into(), vec!["low".into(), "high".into()]);
        assert!(c.effort_supported("gpt-5.5", "high"));
        assert!(!c.effort_supported("gpt-5.5", "ultra"));
        assert!(
            c.effort_supported("gpt-6-astra", "ultra"),
            "not in the catalog, so not doubted"
        );
        assert!(
            c.effort_supported("sonnet", "ultra"),
            "Claude Code has no catalog"
        );
        let parsed: Config = toml::from_str("[agent.codex]\nmodels = [\"x\"]").unwrap();
        assert!(
            parsed.agent.codex.efforts.is_empty(),
            "the file never fills it"
        );
    }

    #[test]
    fn a_model_listed_under_both_backends_stays_with_claude() {
        let c: Config =
            toml::from_str("[agent]\nmodels = [\"shared\"]\n[agent.codex]\nmodels = [\"shared\"]")
                .unwrap();
        assert_eq!(c.agent.backend_of("shared"), CLAUDE_CODE);
    }

    #[test]
    fn a_declared_repository_is_known_by_its_identity_whatever_the_case() {
        let c = Config::parse(
            r#"
            [[repo]]
            github = "slatedb/slatedb"

            [[repo]]
            github = "slatedb/slatedb-go"
            "#,
        )
        .unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(c.declared(), vec!["slatedb/slatedb", "slatedb/slatedb-go"]);
        assert!(c.is_declared("slatedb/slatedb-go"));
        assert!(
            c.is_declared("SlateDB/SlateDB"),
            "the identity is an identity, not a spelling"
        );
        assert!(
            !c.is_declared("slatedb"),
            "the whole owner/repo, not a part of it"
        );
        assert!(!c.is_declared("criccomini/perch"));
        assert!(
            c.repo.iter().all(RepoConfig::is_resolved),
            "github is the identity, nothing to wait for"
        );
    }

    #[test]
    fn a_repository_parses_with_and_without_roots() {
        let c = Config::parse(
            r#"
            [[repo]]
            github = "criccomini/perch"
            roots = ["~/Code/perch", "/tmp/perch-2"]
            base = "trunk"

            [[repo]]
            github = "slatedb/slatedb"
            "#,
        )
        .unwrap();
        assert_eq!(c.repo.len(), 2);
        assert_eq!(
            c.repo[0].root_paths(),
            vec![home_dir().join("Code/perch"), PathBuf::from("/tmp/perch-2")]
        );
        assert_eq!(c.repo[0].base.as_deref(), Some("trunk"));
        assert_eq!(c.roots_for("Criccomini/Perch"), c.repo[0].root_paths());
        assert!(c.repo[1].roots.is_empty());
        assert_eq!(c.repo[1].base, None);
        assert!(c.roots_for("slatedb/slatedb").is_empty());
        assert!(c.roots_for("nobody/declared").is_empty());
    }

    #[test]
    fn the_name_defaults_to_the_repo_half_of_github() {
        let c = Config::parse(
            r#"
            [[repo]]
            github = "slatedb/slatedb"

            [[repo]]
            github = "criccomini/perch"
            name = "Perch"
            "#,
        )
        .unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(c.repo[0].display_name(), "slatedb");
        assert_eq!(
            c.repo[1].display_name(),
            "Perch",
            "a name given is the name shown"
        );
        assert_eq!(
            c.declared_repos(),
            vec![
                DeclaredRepo {
                    identity: "slatedb/slatedb".into(),
                    name: "slatedb".into()
                },
                DeclaredRepo {
                    identity: "criccomini/perch".into(),
                    name: "Perch".into()
                },
            ]
        );
    }

    #[test]
    fn a_roots_only_entry_is_named_after_its_first_root() {
        let c = Config::parse(
            r#"
            [[repo]]
            roots = ["~/Code/notes", "/tmp/notes-2"]

            [[repo]]
            roots = ["/srv/scratch/"]
            name = "Scratch"
            "#,
        )
        .unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(c.repo[0].display_name(), "notes");
        assert_eq!(c.repo[1].display_name(), "Scratch");
        assert_eq!(c.repo[0].github, None);
        assert_eq!(
            c.repo[0].name, None,
            "the default is computed, not written back"
        );
    }

    #[test]
    fn a_roots_only_entry_is_declared_once_resolved() {
        let mut c = Config::parse(
            r#"
            [[repo]]
            github = "slatedb/slatedb"

            [[repo]]
            roots = ["~/Code/notes"]

            [[repo]]
            roots = ["~/Code/perch"]
            "#,
        )
        .unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(c.repo.len(), 3, "the entry is kept, waiting on the scan");
        assert!(!c.repo[1].is_resolved());
        assert_eq!(c.repo[1].identity(), None);
        assert_eq!(
            c.declared(),
            vec!["slatedb/slatedb"],
            "not declared until the scan says what it is"
        );
        assert!(!c.is_declared("local/notes"));
        assert!(c.roots_for("local/notes").is_empty());

        c.repo[1].resolve(None);
        assert_eq!(c.repo[1].identity(), Some("local/notes"));
        assert!(is_local(c.repo[1].identity().unwrap()));
        c.repo[2].resolve(Some("criccomini/perch".into()));
        assert_eq!(c.repo[2].identity(), Some("criccomini/perch"));
        assert!(c.repo.iter().all(RepoConfig::is_resolved));
        assert_eq!(
            c.declared(),
            vec!["slatedb/slatedb", "local/notes", "criccomini/perch"]
        );
        assert!(c.is_declared("Local/Notes"));
        assert_eq!(
            c.roots_for("local/notes"),
            vec![home_dir().join("Code/notes")]
        );
        assert_eq!(
            c.roots_for("criccomini/perch"),
            vec![home_dir().join("Code/perch")]
        );
        assert_eq!(
            c.declared_repos(),
            vec![
                DeclaredRepo {
                    identity: "slatedb/slatedb".into(),
                    name: "slatedb".into()
                },
                DeclaredRepo {
                    identity: "local/notes".into(),
                    name: "notes".into()
                },
                DeclaredRepo {
                    identity: "criccomini/perch".into(),
                    name: "perch".into()
                },
            ]
        );
    }

    #[test]
    fn drop_repeated_identities_drops_the_later_entry_with_a_warning_naming_both() {
        let mut c = Config::parse(
            r#"
            [[repo]]
            github = "slatedb/slatedb"

            [[repo]]
            roots = ["~/Code/slatedb-checkout"]

            [[repo]]
            roots = ["~/Code/notes"]
            "#,
        )
        .unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        c.repo[1].resolve(Some("SlateDB/slatedb".into()));
        c.repo[2].resolve(None);
        c.drop_repeated_identities();
        assert_eq!(c.declared(), vec!["slatedb/slatedb", "local/notes"]);
        assert_eq!(
            c.roots_for("slatedb/slatedb"),
            Vec::<PathBuf>::new(),
            "the first entry's roots, which are none"
        );
        assert_eq!(c.warnings.len(), 1);
        assert!(
            c.warnings[0].contains("[[repo]] roots = [\"~/Code/slatedb-checkout\"]"),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[0].contains("[[repo]] github = \"slatedb/slatedb\""),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[0].ends_with("the later entry was skipped"),
            "{}",
            c.warnings[0]
        );
    }

    #[test]
    fn display_name_finds_the_entry_whatever_the_case_and_echoes_a_stranger() {
        let mut c = Config::parse(
            r#"
            [[repo]]
            github = "slatedb/slatedb"
            name = "SlateDB"

            [[repo]]
            github = "criccomini/perch"

            [[repo]]
            roots = ["~/Code/notes"]
            "#,
        )
        .unwrap();
        c.repo[2].resolve(None);
        assert_eq!(c.display_name("slatedb/slatedb"), "SlateDB");
        assert_eq!(c.display_name("SLATEDB/SLATEDB"), "SlateDB");
        assert_eq!(c.display_name("criccomini/perch"), "perch");
        assert_eq!(c.display_name("Local/Notes"), "notes");
        assert_eq!(
            c.display_name("nobody/declared"),
            "nobody/declared",
            "an undeclared identity is its own name"
        );
    }

    #[test]
    fn a_local_identity_is_one_whose_owner_is_local() {
        assert!(is_local("local/notes"));
        assert!(
            is_local("Local/Notes"),
            "the owner is compared like any other"
        );
        assert!(is_local("LOCAL/x/y"));
        assert!(!is_local("slatedb/slatedb"));
        assert!(!is_local("localhost/x"));
        assert!(!is_local("local"), "no slash, no owner");
        assert!(!is_local(""));
        assert_eq!(LOCAL_OWNER, "local");
    }

    #[test]
    fn github_hosts_is_github_com_and_the_enterprise_host_when_set() {
        let c = Config::parse("").unwrap();
        assert_eq!(c.github_host, None);
        assert_eq!(c.github_hosts(), vec!["github.com"]);
        let c = Config::parse("github_host = \"github.example.com\"").unwrap();
        assert_eq!(c.github_hosts(), vec!["github.com", "github.example.com"]);
        let c = Config::parse("github_host = \"\"").unwrap();
        assert_eq!(
            c.github_hosts(),
            vec!["github.com"],
            "an empty host names nothing"
        );
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
        assert!(
            c.warnings[0].starts_with("config.toml: [[project]] is no longer read"),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[0].contains("[[repo]]"),
            "the message names the new key"
        );
        assert!(
            c.warnings[0].contains("github = \"owner/repo\""),
            "and the key inside it: {}",
            c.warnings[0]
        );
        assert!(c.repo.is_empty());
        assert_eq!(
            c.poll_interval_secs, 60,
            "nothing else in the file was taken either"
        );
    }

    #[test]
    fn a_github_that_is_not_owner_and_repo_is_skipped_with_a_warning() {
        let c = Config::parse(
            r#"
            [[repo]]
            github = "slatedb"
            roots = ["~/Code/slatedb"]

            [[repo]]
            github = "slatedb/slatedb"

            [[repo]]
            github = "a/b/c"

            [[repo]]
            github = "/b"
            "#,
        )
        .unwrap();
        assert_eq!(
            c.declared(),
            vec!["slatedb/slatedb"],
            "the rest of the file loads"
        );
        assert_eq!(c.warnings.len(), 3);
        assert!(
            c.warnings[0].contains("github = \"slatedb\" is not owner/repo"),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[1].contains("github = \"a/b/c\" is not owner/repo"),
            "{}",
            c.warnings[1]
        );
        assert!(
            c.warnings[2].contains("github = \"/b\" is not owner/repo"),
            "{}",
            c.warnings[2]
        );
    }

    #[test]
    fn a_repeated_github_is_skipped_with_a_warning_naming_both() {
        let c = Config::parse(
            r#"
            [[repo]]
            github = "slatedb/slatedb"
            roots = ["~/Code/slatedb"]

            [[repo]]
            github = "slatedb/slatedb-go"

            [[repo]]
            github = "SlateDB/slatedb"
            name = "fork"
            roots = ["~/Code/slatedb", "~/Code/slatedb-fork"]

            [[repo]]
            github = "slatedb/slatedb-go"
            "#,
        )
        .unwrap();
        assert_eq!(
            c.declared(),
            vec!["slatedb/slatedb", "slatedb/slatedb-go"],
            "the first spelling, once"
        );
        assert_eq!(
            c.repo[0].roots,
            vec!["~/Code/slatedb"],
            "the later entry's roots do not join the first's"
        );
        assert!(c.repo[1].roots.is_empty());
        assert_eq!(c.roots_for("SLATEDB/SLATEDB").len(), 1);
        assert_eq!(c.warnings.len(), 2);
        assert!(
            c.warnings[0].contains("[[repo]] github = \"SlateDB/slatedb\" repeats [[repo]] github = \"slatedb/slatedb\""),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[1].contains("[[repo]] github = \"slatedb/slatedb-go\" repeats [[repo]] github = \"slatedb/slatedb-go\""),
            "{}",
            c.warnings[1]
        );
        assert!(
            c.warnings
                .iter()
                .all(|w| w.ends_with("the later entry was skipped")),
            "{:?}",
            c.warnings
        );
    }

    #[test]
    fn a_repeated_name_is_skipped_with_a_warning_naming_both() {
        let c = Config::parse(
            r#"
            [[repo]]
            github = "slatedb/slatedb"

            [[repo]]
            github = "criccomini/slatedb"

            [[repo]]
            roots = ["~/Code/SlateDB"]

            [[repo]]
            github = "criccomini/perch"
            name = "slatedb"

            [[repo]]
            github = "criccomini/slatedb"
            name = "my slatedb"
            "#,
        )
        .unwrap();
        assert_eq!(
            c.declared(),
            vec!["slatedb/slatedb", "criccomini/slatedb"],
            "the first to claim a name keeps it; the last entry's github repeats nothing kept, and its name is its own"
        );
        assert_eq!(
            c.repo.len(),
            2,
            "the roots-only entry went too, not just the GitHub ones"
        );
        assert_eq!(c.display_name("criccomini/slatedb"), "my slatedb");
        assert_eq!(c.warnings.len(), 3, "{:?}", c.warnings);
        assert!(
            c.warnings[0].contains("[[repo]] github = \"criccomini/slatedb\" and [[repo]] github = \"slatedb/slatedb\""),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[0].contains("would both show as \"slatedb\""),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[0].contains("give one of them a name"),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[0].ends_with("The later entry was skipped"),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[1].contains("[[repo]] roots = [\"~/Code/SlateDB\"]"),
            "case does not tell them apart: {}",
            c.warnings[1]
        );
        assert!(
            c.warnings[2].contains("[[repo]] github = \"criccomini/perch\""),
            "a written name counts too: {}",
            c.warnings[2]
        );
    }

    #[test]
    fn the_local_owner_in_github_is_skipped_with_a_warning() {
        let c = Config::parse(
            r#"
            [[repo]]
            github = "local/notes"
            roots = ["~/Code/notes"]

            [[repo]]
            github = "Local/scratch"

            [[repo]]
            github = "slatedb/slatedb"
            "#,
        )
        .unwrap();
        assert_eq!(c.declared(), vec!["slatedb/slatedb"]);
        assert_eq!(c.warnings.len(), 2);
        assert!(
            c.warnings[0].contains("[[repo]] github = \"local/notes\" has the owner local"),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[0].contains("reserved for repositories with no GitHub remote"),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[1].contains("[[repo]] github = \"Local/scratch\""),
            "any case: {}",
            c.warnings[1]
        );
        assert!(
            c.warnings
                .iter()
                .all(|w| w.ends_with("the entry was skipped")),
            "{:?}",
            c.warnings
        );
    }

    #[test]
    fn an_entry_with_neither_github_nor_roots_is_skipped_with_a_warning() {
        let c = Config::parse(
            r#"
            [[repo]]
            name = "notes"

            [[repo]]
            base = "main"

            [[repo]]
            github = "slatedb/slatedb"
            "#,
        )
        .unwrap();
        assert_eq!(c.declared(), vec!["slatedb/slatedb"]);
        assert_eq!(c.repo.len(), 1);
        assert_eq!(c.warnings.len(), 2);
        assert!(
            c.warnings[0]
                .starts_with("config.toml: [[repo]] name = \"notes\" has neither github nor roots"),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[1].starts_with("config.toml: [[repo]] has neither github nor roots"),
            "{}",
            c.warnings[1]
        );
        assert!(
            c.warnings
                .iter()
                .all(|w| w.ends_with("the entry was skipped")),
            "{:?}",
            c.warnings
        );
    }

    #[test]
    fn an_old_name_with_a_slash_is_read_as_github_with_a_notice() {
        let c = Config::parse(
            r#"
            [[repo]]
            name = "slatedb/slatedb"
            roots = ["~/Code/slatedb"]

            [[repo]]
            name = "criccomini/perch"
            github = "criccomini/perch"

            [[repo]]
            name = "a/b/c"
            "#,
        )
        .unwrap();
        assert_eq!(
            c.declared(),
            vec!["slatedb/slatedb", "criccomini/perch"],
            "declared still lists it"
        );
        assert_eq!(c.repo[0].github.as_deref(), Some("slatedb/slatedb"));
        assert_eq!(
            c.repo[0].name, None,
            "the name defaults rather than showing the slug"
        );
        assert_eq!(c.repo[0].display_name(), "slatedb");
        assert_eq!(
            c.roots_for("slatedb/slatedb"),
            vec![home_dir().join("Code/slatedb")]
        );
        assert_eq!(
            c.repo[1].name.as_deref(),
            Some("criccomini/perch"),
            "a name beside github is left as written"
        );
        assert_eq!(c.warnings.len(), 3, "{:?}", c.warnings);
        assert!(
            c.warnings[0].starts_with("config.toml: [[repo]] name = \"slatedb/slatedb\" was read as github = \"slatedb/slatedb\""),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[0].contains("the key is now github, and name is what the sidebar shows"),
            "{}",
            c.warnings[0]
        );
        assert!(
            c.warnings[1].contains("name = \"a/b/c\" was read as github"),
            "{}",
            c.warnings[1]
        );
        assert!(
            c.warnings[2].contains("github = \"a/b/c\" is not owner/repo"),
            "and then judged as github: {}",
            c.warnings[2]
        );
    }

    #[test]
    fn a_malformed_file_is_an_error_not_a_config() {
        assert!(Config::parse("poll_interval_secs = = 3").is_err());
    }
}
