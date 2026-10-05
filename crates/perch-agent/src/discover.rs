//! What is installed, found without being told: where each binary is, and
//! which models Codex will run. Claude Code has stable aliases, so its list
//! is fixed; Codex has only slugs that change with each release, so its
//! own catalog is the list.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Where the CLIs and what they need get installed: the native installers'
/// `~/.local/bin`, then Homebrew, MacPorts and Nix. A Finder launch gets
/// only `/usr/bin:/bin:/usr/sbin:/sbin` on `PATH`, which holds none of them.
pub fn install_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs = Vec::new();
    if let Some(home) = &home {
        dirs.push(home.join(".local/bin"));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"].map(PathBuf::from));
    if let Some(home) = &home {
        dirs.push(home.join(".nix-profile/bin"));
    }
    dirs
}

/// `path` with each of `extra` that exists and is not already on it added
/// at the end, so what the user put first still wins. An npm-installed
/// CLI is a `#!/usr/bin/env node` script, and without this a Finder launch
/// finds the script but not `node`. `None` when there is nothing to change.
///
/// Its relative entries go, an empty one included, which means the
/// current directory. What Perch starts runs in a checkout, a pull
/// request's among them, where `.` or `node_modules/.bin` would find
/// whatever program the pull request put there.
pub fn path_with_install_dirs(path: Option<&OsStr>, extra: &[PathBuf]) -> Option<OsString> {
    let given = path.into_iter().flat_map(std::env::split_paths).collect::<Vec<_>>();
    let mut dirs = given.iter().filter(|dir| dir.is_absolute()).cloned().collect::<Vec<_>>();
    for dir in extra {
        if dir.is_dir() && !dirs.contains(dir) {
            dirs.push(dir.clone());
        }
    }
    (dirs != given).then(|| std::env::join_paths(dirs).ok()).flatten()
}

/// Where a binary is. A name is looked up on `PATH`, then in the
/// directories the two CLIs install into, so an app launched from the
/// Finder with a bare `PATH` still finds them. A path is taken as given,
/// with a leading `~` expanded. `None` means it is not there.
pub fn find_binary(binary: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    find_binary_in(binary, std::env::var_os("PATH").as_deref(), &install_dirs(), home.as_deref())
}

/// `find_binary` with its environment passed in, so it can be tested.
pub fn find_binary_in(binary: &str, path: Option<&OsStr>, extra: &[PathBuf], home: Option<&Path>) -> Option<PathBuf> {
    if binary.contains('/') {
        let expanded = match (binary.strip_prefix("~/"), home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => PathBuf::from(binary),
        };
        return expanded.is_file().then_some(expanded);
    }
    let on_path = path.into_iter().flat_map(std::env::split_paths).collect::<Vec<_>>();
    on_path
        .iter()
        .chain(extra.iter())
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

/// One model Codex offers, from its catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexModel {
    pub slug: String,
    pub display_name: String,
    /// The effort names it takes, as the catalog spells them. Codex
    /// silently runs a level it lacks at the nearest one it has, so the
    /// picker uses this to say so.
    pub efforts: Vec<String>,
}

/// The models Codex lists, in its order. Hidden ones, internal models
/// like its reviewer, are left out. Anything unparseable is skipped, not
/// fatal: the catalog is `codex debug models`, a tool for looking rather
/// than a promise, and a changed shape should cost entries, not the run.
pub fn parse_codex_catalog(json: &str) -> Vec<CodexModel> {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let models = match v.get("models").or(Some(&v)) {
        Some(Value::Array(models)) => models.clone(),
        _ => return Vec::new(),
    };
    models
        .iter()
        .filter(|m| m.get("visibility").and_then(Value::as_str).unwrap_or("list") == "list")
        .filter_map(|m| {
            let slug = m.get("slug").and_then(Value::as_str)?.to_string();
            let display_name =
                m.get("display_name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| slug.clone());
            let efforts = m
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)
                .map(|levels| {
                    levels.iter().filter_map(|l| l.get("effort").and_then(Value::as_str)).map(str::to_string).collect()
                })
                .unwrap_or_default();
            Some(CodexModel { slug, display_name, efforts })
        })
        .collect()
}

/// Ask the binary for its catalog. Half a second on a warm cache, and
/// bounded so a stuck CLI cannot hold the window's startup.
pub async fn codex_catalog(binary: &Path) -> Result<Vec<CodexModel>, String> {
    let run = tokio::process::Command::new(binary)
        .args(["debug", "models"])
        .stdin(std::process::Stdio::null())
        .output();
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), run)
        .await
        .map_err(|_| "codex debug models took more than ten seconds".to_string())?
        .map_err(|e| format!("could not run codex debug models: {e}"))?;
    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("").to_string();
        return Err(format!("codex debug models exited with {}: {tail}", output.status));
    }
    let models = parse_codex_catalog(&String::from_utf8_lossy(&output.stdout));
    if models.is_empty() {
        return Err("codex debug models listed no models".into());
    }
    Ok(models)
}

/// The model Codex itself would run, from its own `config.toml`. The
/// fallback when the catalog cannot be read: one entry the CLI is known
/// to accept.
pub fn codex_default_model() -> Option<String> {
    let home = match std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".codex"),
    };
    codex_default_model_in(&home)
}

pub fn codex_default_model_in(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join("config.toml")).ok()?;
    // The one key, without taking on a TOML parser here: `model = "..."`
    // at the top level, which is where Codex writes it.
    text.lines()
        .map(str::trim)
        .take_while(|l| !l.starts_with('['))
        .find_map(|l| l.strip_prefix("model").map(str::trim_start).and_then(|r| r.strip_prefix('=')))
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = r#"{"models":[
      {"slug":"gpt-6-astra","display_name":"GPT-6-Astra","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"},{"effort":"ultra"}]},
      {"slug":"gpt-reserve","display_name":"GPT-Reserve","visibility":"hide","supported_reasoning_levels":[{"effort":"low"}]},
      {"slug":"gpt-5.5","display_name":"GPT-5.5","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"}]},
      {"display_name":"no slug","visibility":"list"}
    ]}"#;

    #[test]
    fn the_catalog_lists_visible_models_in_order_with_their_efforts() {
        let models = parse_codex_catalog(CATALOG);
        assert_eq!(models.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(), ["gpt-6-astra", "gpt-5.5"]);
        assert_eq!(models[0].display_name, "GPT-6-Astra");
        assert!(models[0].efforts.iter().any(|e| e == "ultra"));
        assert!(!models[1].efforts.iter().any(|e| e == "ultra"));
    }

    #[test]
    fn a_broken_or_strange_catalog_yields_nothing_rather_than_failing() {
        assert!(parse_codex_catalog("not json").is_empty());
        assert!(parse_codex_catalog(r#"{"models": 3}"#).is_empty());
        assert!(parse_codex_catalog(r#"{"other": []}"#).is_empty());
        // A bare list, should the wrapper go.
        assert_eq!(parse_codex_catalog(r#"[{"slug":"x"}]"#)[0].slug, "x");
    }

    #[test]
    fn a_name_is_found_on_path_then_in_the_install_directories() {
        let dir = tempfile::tempdir().unwrap();
        let on_path = dir.path().join("bin");
        let extra = dir.path().join("local");
        std::fs::create_dir_all(&on_path).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(on_path.join("claude"), "").unwrap();
        std::fs::write(extra.join("codex"), "").unwrap();
        let path = std::env::join_paths([&on_path]).unwrap();
        let extras = [extra.clone()];

        assert_eq!(find_binary_in("claude", Some(&path), &extras, None), Some(on_path.join("claude")));
        assert_eq!(find_binary_in("codex", Some(&path), &extras, None), Some(extra.join("codex")));
        assert_eq!(find_binary_in("gemini", Some(&path), &extras, None), None);
        assert_eq!(find_binary_in("claude", None, &[], None), None, "no PATH, no extras, nothing");
    }

    #[test]
    fn install_dirs_go_after_the_path_once_each_and_only_if_they_exist() {
        let dir = tempfile::tempdir().unwrap();
        let on_path = dir.path().join("bin");
        let brew = dir.path().join("brew");
        std::fs::create_dir_all(&on_path).unwrap();
        std::fs::create_dir_all(&brew).unwrap();
        let missing = dir.path().join("missing");
        let path = std::env::join_paths([&on_path]).unwrap();
        let extra = [on_path.clone(), missing, brew.clone()];

        let joined = path_with_install_dirs(Some(&path), &extra).unwrap();
        assert_eq!(std::env::split_paths(&joined).collect::<Vec<_>>(), [on_path.clone(), brew.clone()]);
        assert_eq!(path_with_install_dirs(Some(&joined), &extra), None, "nothing new to add");
        assert_eq!(path_with_install_dirs(None, &[brew.clone()]), Some(brew.into_os_string()));
    }

    #[test]
    fn relative_path_entries_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let on_path = dir.path().join("bin");
        std::fs::create_dir_all(&on_path).unwrap();
        // `::` is an empty entry, the current directory.
        let path = format!(".:node_modules/.bin:{}::", on_path.display());
        let kept = path_with_install_dirs(Some(OsStr::new(&path)), &[]).unwrap();
        assert_eq!(std::env::split_paths(&kept).collect::<Vec<_>>(), [on_path]);
        assert_eq!(path_with_install_dirs(Some(&kept), &[]), None, "nothing left to drop");
    }

    #[test]
    fn a_path_is_taken_as_given_with_the_home_expanded() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("tools").join("codex");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "").unwrap();

        assert_eq!(find_binary_in(bin.to_str().unwrap(), None, &[], None), Some(bin.clone()));
        assert_eq!(find_binary_in("~/tools/codex", None, &[], Some(dir.path())), Some(bin.clone()));
        assert_eq!(find_binary_in("~/tools/nope", None, &[], Some(dir.path())), None);
        assert_eq!(find_binary_in(dir.path().to_str().unwrap(), None, &[], None), None, "a directory is not a binary");
    }

    #[test]
    fn codex_s_own_default_model_is_read_from_its_config() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(codex_default_model_in(home.path()), None);
        std::fs::write(home.path().join("config.toml"), "model = \"gpt-6-astra\"\nmodel_reasoning_effort = \"low\"\n[projects]\nmodel = \"other\"\n").unwrap();
        assert_eq!(codex_default_model_in(home.path()).as_deref(), Some("gpt-6-astra"));
        std::fs::write(home.path().join("config.toml"), "[agent]\nmodel = \"only-in-a-table\"\n").unwrap();
        assert_eq!(codex_default_model_in(home.path()), None);
    }

    #[tokio::test]
    async fn the_catalog_comes_from_the_binary_and_a_failure_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("codex");
        std::fs::write(&good, format!("#!/bin/sh\ncat <<'EOF'\n{CATALOG}\nEOF\n")).unwrap();
        let bad = dir.path().join("codex-bad");
        std::fs::write(&bad, "#!/bin/sh\necho 'no such command' >&2\nexit 2\n").unwrap();
        for b in [&good, &bad] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(b, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let models = codex_catalog(&good).await.unwrap();
        assert_eq!(models.len(), 2);
        let err = codex_catalog(&bad).await.unwrap_err();
        assert!(err.contains("exited") && err.contains("no such command"), "{err}");
        let err = codex_catalog(&dir.path().join("missing")).await.unwrap_err();
        assert!(err.contains("could not run"), "{err}");
    }
}
