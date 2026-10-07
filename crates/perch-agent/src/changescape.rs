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

/// The commands the skill runs.
const SKILL_COMMANDS: &[&str] = &[
    "Bash(uv run *)",
    "Bash(gh pr *)",
    "Bash(gh api *)",
    "Bash(gh issue *)",
    "Bash(git *)",
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
    /// Perch's clone of the repository.
    pub cwd: PathBuf,
    /// Changescape's cache, which the shell may write.
    pub cache_root: PathBuf,
    /// The one folder the run's own Write and Edit may touch.
    pub build_dir: PathBuf,
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
            settings(&self.cwd, &self.cache_root),
            "--add-dir".into(),
            real_path(&self.cache_root).to_string_lossy().into_owned(),
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
        ];
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

/// The sandbox: writes to the cache, the clone and the temporary
/// directory; GitHub and the package index on the network. `uv` keeps its
/// cache and the skill's environment under the cache root, the only
/// place it can write.
pub fn settings(cwd: &Path, cache_root: &Path) -> String {
    let cache = real_path(cache_root);
    let uv = cache.join(".uv");
    serde_json::json!({
        "sandbox": {
            "enabled": true,
            "autoAllowBashIfSandboxed": false,
            "allowUnsandboxedCommands": false,
            "failIfUnavailable": true,
            "filesystem": {
                "allowWrite": [cache, real_path(cwd), real_path(&std::env::temp_dir())],
            },
            "network": {
                "allowedDomains": DOMAINS,
                "strictAllowlist": true,
            },
        },
        "env": {
            "UV_CACHE_DIR": uv.join("cache"),
            "UV_PROJECT_ENVIRONMENT": uv.join("venv"),
        },
    })
    .to_string()
}

impl Run {
    /// Start a Changescape build. Its events come as a question's do.
    pub fn spawn_changescape(
        spec: ChangescapeSpec,
        events: UnboundedSender<RunEvent>,
    ) -> Result<Run, Error> {
        let args = spec.args();
        Self::spawn_process(
            Backend::ClaudeCode,
            spec.binary,
            spec.cwd,
            args,
            None,
            None,
            events,
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
            cwd: "/tmp/repos/a.git".into(),
            cache_root: "/tmp/cs".into(),
            build_dir: "/tmp/cs/o/r/pr-1/abc".into(),
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
