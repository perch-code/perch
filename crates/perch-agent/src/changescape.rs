//! A Changescape build: Claude Code running the Changescape skill on one
//! pull request, in Perch's own clone of the repository, writing its pages
//! into Changescape's cache.
//!
//! It differs from a question in three ways. Safe mode is off, since safe
//! mode turns skills off. The tools are the skill's: reading, writing in
//! the build's folder alone, and the commands the skill runs. And the
//! sandbox lets the shell write to the cache and to the clone, which the
//! skill fetches the pull request's head into, and reach GitHub and the
//! Python package index that `uv` installs the skill's tools from.

use std::path::{Path, PathBuf};

use tokio::sync::mpsc::UnboundedSender;

use crate::{real_path, Backend, Error, Run, RunEvent};

/// The commands the skill runs. Its steps run from its own folder, so
/// Claude often starts them with a `cd` there; each part of a compound
/// command needs a rule.
const SKILL_COMMANDS: &[&str] = &[
    "Bash(cd *)",
    "Bash(uv run *)",
    "Bash(gh pr *)",
    "Bash(gh issue *)",
    "Bash(git *)",
    // What Claude pipes a step's output through. The sandbox, not this
    // list, is what keeps the build to its folders.
    "Bash(tail *)",
    "Bash(head *)",
    "Bash(cat *)",
    "Bash(ls *)",
    "Bash(wc *)",
    "Bash(grep *)",
    "Bash(sort *)",
    "Bash(jq *)",
    "Bash(echo *)",
];

/// What `gh`, `git fetch` and `uv` reach.
const DOMAINS: &[&str] = &[
    "api.github.com",
    "github.com",
    "pypi.org",
    "files.pythonhosted.org",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangescapeSpec {
    pub binary: PathBuf,
    pub model: String,
    /// Perch's clone of the repository, which the skill fetches the pull
    /// request's head into.
    pub repo: PathBuf,
    /// Changescape's cache, which the shell may write, and where the run
    /// runs. Not the clone: the sandbox refuses every write to the git
    /// internals of the folder a run runs in, and a bare clone is all
    /// internals, so the skill's fetch could not store a thing.
    pub cache_root: PathBuf,
    /// The one folder the run's own Write and Edit may touch.
    pub build_dir: PathBuf,
    /// The skill's folder, which its steps `cd` into.
    pub skill_dir: Option<PathBuf>,
    /// Perch's GitHub token, as `GH_TOKEN`: `gh` in the sandbox cannot
    /// reach the keychain its own login is in. Every other run is denied
    /// tokens; this one reads pull requests with `gh pr` and `gh issue`,
    /// and is not given `gh api`.
    pub github_token: Option<String>,
    pub prompt: String,
}

impl ChangescapeSpec {
    /// The command line, for the log and for tests.
    pub fn args(&self) -> Vec<String> {
        let build = real_path(&self.build_dir);
        let build = build.to_string_lossy();
        // An absolute path in a permission rule starts with `//`.
        let under_build = format!("/{}/**", build.trim_end_matches('/'));
        let mut args: Vec<String> = vec![
            "--print".into(),
            "--strict-mcp-config".into(),
            "--no-session-persistence".into(),
            "--settings".into(),
            settings(&self.repo, &self.cache_root),
            "--add-dir".into(),
            real_path(&self.repo).to_string_lossy().into_owned(),
        ];
        // `--add-dir` takes every value up to the next option, so the
        // skill's folder goes before one.
        if let Some(skill) = &self.skill_dir {
            args.push(real_path(skill).to_string_lossy().into_owned());
        }
        args.extend([
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--model".into(),
            self.model.clone(),
            "--permission-mode".into(),
            "dontAsk".into(),
            "--permission-prompts".into(),
            "none".into(),
            "--allowedTools".into(),
            // The skill is reached through the Skill tool.
            "Skill".into(),
            "Read".into(),
            "Grep".into(),
            "Glob".into(),
            format!("Write({under_build})"),
            format!("Edit({under_build})"),
        ]);
        args.extend(SKILL_COMMANDS.iter().map(|c| (*c).to_string()));
        // Everything not allowed is refused under `dontAsk`; the network
        // tools are named too, as every run names them.
        args.push("--disallowedTools".into());
        args.extend(["WebFetch".to_string(), "WebSearch".to_string()]);
        args.push("--".into());
        args.push(self.prompt.clone());
        args
    }
}

/// The sandbox: writes to the cache, the clone, the temporary directory
/// and `uv`'s cache; GitHub and the package index on the network.
///
/// `uv` runs offline, from the packages the reader's own runs of the skill
/// left in its cache. Online, it panics in the sandbox: reading the
/// system's proxy configuration is denied ("Attempted to create a NULL
/// object", uv 0.8.19), so a package not in the cache cannot be fetched.
pub fn settings(repo: &Path, cache_root: &Path) -> String {
    let cache = real_path(cache_root);
    let uv_cache = std::env::var_os("UV_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/uv")))
        .map(|dir| real_path(&dir));
    let allow_write: Vec<PathBuf> = [cache, real_path(repo), real_path(&std::env::temp_dir())]
        .into_iter()
        .chain(uv_cache)
        .collect();
    serde_json::json!({
        "sandbox": {
            "enabled": true,
            "autoAllowBashIfSandboxed": false,
            "allowUnsandboxedCommands": false,
            "failIfUnavailable": true,
            // Go's TLS on macOS, `gh`'s and `git`'s helpers among it,
            // checks certificates with the system's trust service, which
            // the sandbox otherwise denies.
            "enableWeakerNetworkIsolation": true,
            "filesystem": {
                "allowWrite": allow_write,
            },
            "network": {
                "allowedDomains": DOMAINS,
                "strictAllowlist": true,
            },
        },
        "env": {
            "UV_OFFLINE": "1",
        },
    })
    .to_string()
}

/// The token for `gh`, and for the skill's `git fetch` over HTTPS the
/// credential helper Perch's own fetches use, read from the same
/// variable and scoped to github.com. The empty value first clears any
/// helper the reader's configuration has for it.
fn token_env(token: String) -> Vec<(String, String)> {
    let helper = "credential.https://github.com.helper";
    [
        ("GH_TOKEN", token),
        ("GIT_CONFIG_COUNT", "2".into()),
        ("GIT_CONFIG_KEY_0", helper.into()),
        ("GIT_CONFIG_VALUE_0", String::new()),
        ("GIT_CONFIG_KEY_1", helper.into()),
        (
            "GIT_CONFIG_VALUE_1",
            "!f() { echo username=x-access-token; echo password=$GH_TOKEN; }; f".into(),
        ),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

impl Run {
    /// Start a Changescape build. Its events come as a question's do.
    pub fn spawn_changescape(
        spec: ChangescapeSpec,
        events: UnboundedSender<RunEvent>,
    ) -> Result<Run, Error> {
        let args = spec.args();
        let env = spec.github_token.map(token_env).unwrap_or_default();
        Self::spawn_process(
            Backend::ClaudeCode,
            spec.binary,
            spec.cache_root,
            args,
            None,
            None,
            events,
            env,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ChangescapeSpec {
        ChangescapeSpec {
            binary: "claude".into(),
            model: "opus".into(),
            repo: "/tmp/repos/a.git".into(),
            cache_root: "/tmp/cs".into(),
            build_dir: "/tmp/cs/o/r/pr-1/abc".into(),
            skill_dir: Some("/tmp/skills/changescape".into()),
            github_token: None,
            prompt: "Use the changescape skill".into(),
        }
    }

    #[test]
    fn safe_mode_is_off_and_the_rest_of_isolation_stays() {
        let args = spec().args();
        assert!(!args.iter().any(|a| a == "--safe-mode"));
        for kept in [
            "--print",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--verbose",
            "dontAsk",
        ] {
            assert!(args.iter().any(|a| a == kept), "{kept}");
        }
        assert_eq!(args.last().unwrap(), "Use the changescape skill");
        assert_eq!(args[args.len() - 2], "--");
    }

    #[test]
    fn writes_are_the_build_folder_only() {
        let args = spec().args();
        let writes: Vec<&String> = args
            .iter()
            .filter(|a| a.starts_with("Write(") || a.starts_with("Edit("))
            .collect();
        assert_eq!(writes.len(), 2);
        assert!(
            writes.iter().all(|w| w.contains("pr-1/abc/**")),
            "{writes:?}"
        );
        assert!(
            writes.iter().all(|w| w.contains("(//")),
            "absolute: {writes:?}"
        );
    }

    #[test]
    fn the_sandbox_reaches_github_and_the_package_index_only() {
        let settings: serde_json::Value =
            serde_json::from_str(&settings(Path::new("/tmp"), Path::new("/tmp"))).unwrap();
        let domains = &settings["sandbox"]["network"]["allowedDomains"];
        assert_eq!(domains.as_array().unwrap().len(), 4);
        assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
    }
}
