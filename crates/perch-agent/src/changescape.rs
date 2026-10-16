//! A Changescape build: Claude Code running the Changescape skill on one
//! pull request, writing its pages into Changescape's cache. Safe mode is
//! off, since it turns skills off.

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
    "Bash(sed *)",
    // How Claude reads the skill's JSON in passing. It can do no more
    // than `uv run` already can, inside the same sandbox.
    "Bash(python3 *)",
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
    /// Changescape's cache: the shell may write here, and the run runs
    /// here, not in the bare clone, whose git internals the sandbox will
    /// not let a run write.
    pub cache_root: PathBuf,
    /// The one folder the run's own Write and Edit may touch.
    pub build_dir: PathBuf,
    /// The skill's folder, which its steps `cd` into.
    pub skill_dir: Option<PathBuf>,
    /// Perch's GitHub token, as `GH_TOKEN`, since the sandbox hides `gh`'s
    /// keychain login.
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
        args.push("--disallowedTools".into());
        args.extend(["WebFetch".to_string(), "WebSearch".to_string()]);
        args.push("--".into());
        args.push(self.prompt.clone());
        args
    }
}

/// The sandbox: writes to the cache, the clone, the temporary directory
/// and `uv`'s cache; GitHub and the package index on the network.
/// `uv` runs offline from the reader's own cache, and does not sync the
/// skill's environment: a sync makes an HTTP client, which reads the
/// system's proxy configuration, and the sandbox denies that, so `uv`
/// panics, offline or not. Perch syncs the skill before the run instead.
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
            "UV_NO_SYNC": "1",
        },
    })
    .to_string()
}

/// `GH_TOKEN`, and a github.com credential helper for `git` that reads
/// it, after an empty helper that clears the reader's own.
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
    fn the_sandbox_writes_the_cache_and_the_clone_and_reaches_github_and_pypi() {
        let (repo, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let settings: serde_json::Value =
            serde_json::from_str(&settings(repo.path(), cache.path())).unwrap();
        let sandbox = &settings["sandbox"];
        assert_eq!(
            sandbox["network"]["allowedDomains"],
            serde_json::json!([
                "api.github.com",
                "github.com",
                "pypi.org",
                "files.pythonhosted.org"
            ])
        );
        assert_eq!(sandbox["network"]["strictAllowlist"], true);
        let writes: Vec<PathBuf> =
            serde_json::from_value(sandbox["filesystem"]["allowWrite"].clone()).unwrap();
        for dir in [cache.path(), repo.path(), std::env::temp_dir().as_path()] {
            assert!(writes.contains(&real_path(dir)), "{dir:?} in {writes:?}");
        }
        assert_eq!(settings["env"]["UV_OFFLINE"], "1");
        assert_eq!(settings["env"]["UV_NO_SYNC"], "1");
    }

    #[test]
    fn the_shell_runs_the_skills_commands_and_no_web_tools() {
        let args = spec().args();
        let allowed = args.iter().position(|a| a == "--allowedTools").unwrap();
        let disallowed = args.iter().position(|a| a == "--disallowedTools").unwrap();
        let bash: Vec<&str> = args[allowed..disallowed]
            .iter()
            .filter(|a| a.starts_with("Bash("))
            .map(String::as_str)
            .collect();
        assert_eq!(bash, SKILL_COMMANDS);
        assert_eq!(
            args[disallowed + 1..disallowed + 3],
            ["WebFetch", "WebSearch"]
        );
    }

    #[test]
    fn the_skill_folder_is_an_added_dir_before_the_next_option() {
        let args = spec().args();
        let at = args.iter().position(|a| a == "--add-dir").unwrap();
        assert_eq!(
            args[at + 2],
            real_path(Path::new("/tmp/skills/changescape")).to_string_lossy()
        );
        assert!(args[at + 3].starts_with("--"));
    }

    #[test]
    fn the_token_helper_clears_the_readers_own_first() {
        let env: std::collections::HashMap<String, String> =
            token_env("t0ken".into()).into_iter().collect();
        assert_eq!(env["GH_TOKEN"], "t0ken");
        assert_eq!(env["GIT_CONFIG_COUNT"], "2");
        let helper = "credential.https://github.com.helper";
        assert_eq!(
            (
                env["GIT_CONFIG_KEY_0"].as_str(),
                env["GIT_CONFIG_VALUE_0"].as_str()
            ),
            (helper, "")
        );
        assert_eq!(env["GIT_CONFIG_KEY_1"], helper);
        // The helper reads the variable; the token itself is not in git's
        // configuration.
        assert!(env["GIT_CONFIG_VALUE_1"].contains("password=$GH_TOKEN"));
        assert!(!env["GIT_CONFIG_VALUE_1"].contains("t0ken"));
    }
}
