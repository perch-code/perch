//! OpenCode 2.x over its private ACP server. A named mode must be selected
//! successfully before sending any prompt, including on resumed sessions.
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::process::Command;

use crate::gemini::{AcpClient, AcpSession};
use crate::{real_path, Backend, Conversation};

pub const MIN_OPENCODE_VERSION: crate::CliVersion = crate::CliVersion::new(2, 3, 0);

const VCS_MARKERS: &[&str] = &[".git", ".hg", ".jj", ".repo", ".citc"];
const SCAN_LIMIT: usize = 200_000;

struct Finding {
    path: PathBuf,
    says: &'static str,
}

pub(crate) fn conversation(
    cwd: &Path,
    model: &str,
    effort: &str,
    prompt: String,
    resume: Option<String>,
    writable: Vec<PathBuf>,
    read_only: Vec<PathBuf>,
) -> Conversation {
    Conversation::Acp(Box::new(AcpClient::new(AcpSession {
        backend: Backend::OpenCode,
        cwd: real_path(cwd),
        model: model.into(),
        effort: effort.into(),
        prompt,
        resume,
        writable,
        read_only,
    })))
}

pub(crate) fn mode(work: bool) -> &'static str {
    if work {
        "perch-work"
    } else {
        "perch-read"
    }
}

fn perch_home() -> Result<PathBuf, String> {
    perch_home_in(
        std::env::var_os("PERCH_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn perch_home_in(
    perch_home_env: Option<&OsStr>,
    home_env: Option<&OsStr>,
) -> Result<PathBuf, String> {
    let home = home_env.filter(|h| !h.is_empty()).map(PathBuf::from);
    if let Some(val) = perch_home_env.filter(|v| !v.is_empty()) {
        let path = match (Path::new(val).strip_prefix("~"), home.as_deref()) {
            (Ok(rest), Some(home)) if rest.as_os_str().is_empty() => home.to_path_buf(),
            (Ok(rest), Some(home)) => home.join(rest),
            _ => PathBuf::from(val),
        };
        if !path.is_absolute() {
            return Err("PERCH_HOME must be an absolute path".into());
        }
        return Ok(path);
    }
    match home.filter(|h| h.is_absolute()) {
        Some(home) => Ok(home.join(".perch")),
        None => Err("OpenCode requires HOME or PERCH_HOME".into()),
    }
}

fn slash_path(path: &Path) -> Option<String> {
    let raw = path.to_string_lossy().replace('\\', "/");
    let trimmed = if raw == "/" {
        "/"
    } else {
        raw.trim_end_matches('/')
    };
    let is_abs = trimmed.starts_with('/')
        || (trimmed.len() >= 3
            && trimmed.as_bytes()[0].is_ascii_alphabetic()
            && trimmed.as_bytes()[1] == b':'
            && trimmed.as_bytes()[2] == b'/');
    if !is_abs || trimmed == "/" || trimmed.contains('*') || trimmed.contains('?') {
        return None;
    }
    Some(trimmed.to_string())
}

fn config_value(work: bool, external_reads: &[PathBuf], read_only: &[PathBuf]) -> Value {
    let rule = |action: &str, resource: &str, effect: &str| json!({"action": action, "resource": resource, "effect": effect});
    let gate = if work { "ask" } else { "deny" };
    let mut rules = vec![
        rule("*", "*", "deny"),
        rule("glob", "*", "allow"),
        rule("grep", "*", "allow"),
        rule("read", "*", "allow"),
        rule("read", "/*", gate),
        rule("read", "?:/*", gate),
    ];
    if work {
        rules.push(rule("external_directory", "*", "ask"));
    }
    let push_unique = |rules: &mut Vec<Value>, item: Value| {
        if !rules.contains(&item) {
            rules.push(item);
        }
    };
    for path in external_reads {
        let real = real_path(path);
        let Some(slash) = slash_path(&real) else {
            continue;
        };
        if real.is_file() {
            if let Some(parent_slash) = real.parent().and_then(slash_path) {
                push_unique(
                    &mut rules,
                    rule("external_directory", &format!("{parent_slash}/*"), "allow"),
                );
            }
            push_unique(&mut rules, rule("read", &slash, "allow"));
        } else {
            let children = format!("{slash}/*");
            push_unique(&mut rules, rule("external_directory", &slash, "allow"));
            push_unique(&mut rules, rule("external_directory", &children, "allow"));
            push_unique(&mut rules, rule("read", &slash, "allow"));
            push_unique(&mut rules, rule("read", &children, "allow"));
        }
    }
    rules.push(rule("read", "*.env", gate));
    rules.push(rule("read", "*.env.*", gate));
    rules.push(rule("read", "*.env.example", "allow"));
    if work {
        for tool in ["edit", "shell", "webfetch", "websearch"] {
            rules.push(rule(tool, "*", "ask"));
        }
        for path in read_only {
            if let Some(slash) = slash_path(path) {
                rules.push(rule("edit", &slash, "deny"));
                rules.push(rule("edit", &format!("{slash}/*"), "deny"));
            }
        }
    }
    json!({
        "agents": { (mode(work)): {"mode": "primary", "permissions": rules} },
        "policies": [
            rule("plugin.use", "*", "deny"),
            rule("mcp.use", "*", "deny"),
        ],
        "snapshots": false,
        "lsp": false,
        "formatter": false,
    })
}

pub(crate) fn configure(
    command: &mut Command,
    work: bool,
    external_reads: &[PathBuf],
    read_only: &[PathBuf],
) -> Result<(), String> {
    let home = perch_home()?;
    configure_in(command, &home, work, external_reads, read_only)
}

fn configure_in(
    command: &mut Command,
    home: &Path,
    work: bool,
    external_reads: &[PathBuf],
    read_only: &[PathBuf],
) -> Result<(), String> {
    // OpenCode applies previously saved "always allow" decisions after
    // agent-level "ask" rules. Keep Perch sessions in their own database
    // under PERCH_HOME without overriding XDG_DATA_HOME, which would leak
    // into child commands spawned by OpenCode's shell tool.
    let opencode_dir = home.join("opencode");
    let config_dir = opencode_dir.join("config");
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("could not create {}: {e}", config_dir.display()))?;

    let config = config_value(work, external_reads, read_only);
    if let Some(key) = std::env::var_os("OPENCODE_API_KEY") {
        command.env("OPENCODE_API_KEY", key);
    }
    command
        .current_dir("/")
        .env("OPENCODE_DB", opencode_dir.join("opencode.db"))
        .env_remove("OPENCODE_CONFIG")
        .env("OPENCODE_CONFIG_DIR", config_dir)
        .env("OPENCODE_CONFIG_PROJECT_DISABLE", "1")
        .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
        .env("OPENCODE_DISABLE_CLAUDE_CODE", "1")
        .env("OPENCODE_CONFIG_CONTENT", config.to_string());
    Ok(())
}

/// Refuse an untrusted checkout whose `AGENTS.md` or `CLAUDE.md` symlinks
/// point outside the repository: OpenCode's `ReadTool` resolves symlinks on
/// ancestor instruction files and loads them into the prompt without
/// consulting `FileAccess.authorizeRead`.
pub(crate) fn refusal(cwd: &Path, added: &[PathBuf], trusted: bool) -> Option<String> {
    if trusted {
        return None;
    }
    refusal_with_budget(cwd, added, SCAN_LIMIT)
}

fn refusal_with_budget(cwd: &Path, added: &[PathBuf], mut budget: usize) -> Option<String> {
    let starts = std::iter::once((cwd, true)).chain(added.iter().map(|d| (d.as_path(), false)));
    for (start, own) in starts {
        if let Some(finding) = vet_tree(start, &mut budget) {
            return Some(refusal_text(cwd, &finding, own));
        }
    }
    None
}

pub(crate) fn can_read_archive(dir: &Path) -> bool {
    slash_path(&real_path(dir)).is_some() && refusal(dir, &[], false).is_none()
}

pub(crate) fn can_add_optional_read_dir(cwd: &Path, added: &[PathBuf], dir: &Path) -> bool {
    can_add_read_dir_with_budget(cwd, added, dir, SCAN_LIMIT)
}

fn can_add_read_dir_with_budget(cwd: &Path, added: &[PathBuf], dir: &Path, budget: usize) -> bool {
    if slash_path(&real_path(dir)).is_none() {
        return false;
    }
    let mut dirs = added.to_vec();
    dirs.push(dir.to_path_buf());
    refusal_with_budget(cwd, &dirs, budget).is_none()
}

fn refusal_text(cwd: &Path, finding: &Finding, own: bool) -> String {
    let real = real_path(cwd);
    let shown = match finding.path.strip_prefix(&real) {
        Ok(inside) if !inside.as_os_str().is_empty() => inside.display().to_string(),
        _ => crate::prompt_path(&finding.path),
    };
    let what = finding.says.replace("{path}", &shown);
    if own {
        format!(
            "{what}. So Perch does not run OpenCode here. OpenCode can still work here, as Work, \
once the repository is trusted in config.toml (`[repo.work] trust = true`); a question or a \
review can't."
        )
    } else {
        format!(
            "{what}. This run also reads that directory, so Perch does not run OpenCode here \
unless the repository is trusted for Work in config.toml (`[repo.work] trust = true`); a question \
or a review can't."
        )
    }
}

fn walked_ancestors(real_start: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in real_start.ancestors() {
        dirs.push(dir.to_path_buf());
        if VCS_MARKERS
            .iter()
            .any(|marker| dir.join(marker).symlink_metadata().is_ok())
        {
            return dirs;
        }
    }
    vec![real_start.to_path_buf()]
}

fn is_instruction_name(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|s| s.eq_ignore_ascii_case("agents.md") || s.eq_ignore_ascii_case("claude.md"))
}

fn vet_tree(start: &Path, budget: &mut usize) -> Option<Finding> {
    let root = real_path(start);
    if !root.is_dir() {
        return None;
    }
    let ancestors = walked_ancestors(&root);
    let vcs_root = ancestors.last().unwrap_or(&root);
    for ancestor in ancestors.iter().skip(1) {
        if let Some(finding) = vet_dir_instructions(ancestor, vcs_root, budget, None) {
            return Some(finding);
        }
    }
    let mut queue = VecDeque::from([root.clone()]);
    let mut children = Vec::new();
    while let Some(dir) = queue.pop_front() {
        children.clear();
        if let Some(finding) = vet_dir_instructions(&dir, &root, budget, Some(&mut children)) {
            return Some(finding);
        }
        children.sort();
        queue.extend(children.drain(..));
    }
    None
}

fn vet_dir_instructions(
    dir: &Path,
    root: &Path,
    budget: &mut usize,
    mut subdirs: Option<&mut Vec<PathBuf>>,
) -> Option<Finding> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            return Some(Finding {
                path: dir.to_path_buf(),
                says: "Perch can't read {path} to check what OpenCode would load from it",
            });
        }
    };
    let mut links = Vec::new();
    for entry in entries {
        if *budget == 0 {
            return Some(Finding {
                path: dir.to_path_buf(),
                says: "{path} has too many entries for Perch to check what OpenCode would load from it",
            });
        }
        *budget -= 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                return Some(Finding {
                    path: dir.to_path_buf(),
                    says: "Perch can't read {path} to check what OpenCode would load from it",
                });
            }
        };
        let name = entry.file_name();
        if name.eq_ignore_ascii_case(".git") {
            continue;
        }
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => {
                return Some(Finding {
                    path: entry.path(),
                    says: "Perch can't read {path} to check what OpenCode would load from it",
                });
            }
        };
        if ft.is_dir() {
            if let Some(subdirs) = subdirs.as_deref_mut() {
                subdirs.push(entry.path());
            }
        } else if ft.is_symlink() && is_instruction_name(&name) {
            links.push(entry.path());
        }
    }
    links.sort();
    for path in links {
        if let Ok(target) = std::fs::canonicalize(&path) {
            if !target.starts_with(root) {
                return Some(Finding {
                    path,
                    says: "{path} links to a file outside the repository, which OpenCode would load into the prompt",
                });
            }
        }
    }
    None
}

/// OpenCode lists one provider-qualified model ID on each line.
pub async fn opencode_catalog(binary: &Path) -> Result<Vec<String>, String> {
    let mut cmd = Command::new(binary);
    crate::scrub_env(&mut cmd);
    configure(&mut cmd, false, &[], &[])?;
    cmd.args(["models", "--standalone"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .map_err(|_| "opencode models took more than ten seconds".to_string())?
        .map_err(|e| format!("could not run opencode models: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "opencode models exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("")
        ));
    }
    let models: Vec<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| {
            !s.contains(char::is_whitespace)
                && s.split_once('/')
                    .is_some_and(|(p, m)| !p.is_empty() && !m.is_empty())
        })
        .map(str::to_owned)
        .collect();
    if models.is_empty() {
        return Err("opencode models listed no models".into());
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::Reply;

    fn request(reply: Reply) -> Value {
        serde_json::from_str(&reply.writes[0]).unwrap()
    }

    fn wildcard_match(pattern: &str, text: &str) -> bool {
        fn go(p: &[char], t: &[char]) -> bool {
            match (p.first(), t.first()) {
                (None, None) => true,
                (Some('*'), _) => go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..])),
                (Some('?'), Some(_)) => go(&p[1..], &t[1..]),
                (Some(pc), Some(tc)) if pc == tc => go(&p[1..], &t[1..]),
                _ => false,
            }
        }
        let p: Vec<char> = pattern.chars().collect();
        let t: Vec<char> = text.chars().collect();
        go(&p, &t)
    }

    fn eval_effect<'a>(rules: &'a [Value], action: &str, resource: &str) -> &'a str {
        rules
            .iter()
            .filter(|r| {
                wildcard_match(r["action"].as_str().unwrap(), action)
                    && wildcard_match(r["resource"].as_str().unwrap(), resource)
            })
            .next_back()
            .and_then(|r| r["effect"].as_str())
            .unwrap_or("deny")
    }

    #[test]
    fn a_resumed_read_only_run_sets_the_model_effort_and_mode_before_prompting() {
        let mut client = conversation(
            Path::new("/"),
            "provider/model",
            "high",
            "Follow up".into(),
            Some("s1".into()),
            vec![],
            vec![],
        );
        let resume =
            request(client.after_line(&json!({"id":1,"result":{"agentInfo":{"version":"2.3.0"}}})));
        assert_eq!(resume["method"], "session/resume");
        assert_eq!(resume["params"]["sessionId"], "s1");
        let model = request(client.after_line(&json!({"id":2,"result":{}})));
        assert_eq!(model["params"]["value"], "provider/model");
        let effort = request(client.after_line(&json!({"id":3,"result":{"configOptions":[
            {"id":"effort","options":[{"value":"high"}]}]}})));
        assert_eq!(effort["params"]["configId"], "effort");
        assert_eq!(effort["params"]["value"], "high");
        let mode = request(client.after_line(&json!({"id":4,"result":{}})));
        assert_eq!(mode["params"]["value"], "perch-read");
        let prompt = request(client.after_line(&json!({"id":5,"result":{}})));
        assert_eq!(prompt["params"]["prompt"][0]["text"], "Follow up");
        let refused = request(client.after_line(&json!({"id":22,"method":"session/request_permission",
            "params":{"toolCall":{"kind":"execute"}, "options":[{"optionId":"no","kind":"reject_once"}]}})));
        assert_eq!(refused["result"]["outcome"]["optionId"], "no");
    }

    #[test]
    fn a_cli_without_the_required_mode_never_receives_the_prompt() {
        let mut client = conversation(
            Path::new("/"),
            "provider/model",
            "low",
            "Edit".into(),
            None,
            vec![PathBuf::from("/work")],
            vec![],
        );
        let mode = request(client.after_line(&json!({"id":3,"result":{"configOptions":[]}})));
        assert_eq!(mode["params"]["value"], "perch-work");
        let rejected = client.after_line(&json!({"id":5,"error":{"message":"unknown mode"}}));
        assert!(rejected.close && rejected.writes.is_empty());
        assert!(
            matches!(&rejected.events[..], [crate::RunEvent::Failed{message}] if message == "unknown mode")
        );
    }

    #[test]
    fn perch_home_resolves_override_tilde_and_fallback_without_touching_xdg_data_home() {
        assert_eq!(
            perch_home_in(Some(OsStr::new("/srv/perch")), None).unwrap(),
            PathBuf::from("/srv/perch")
        );
        assert_eq!(
            perch_home_in(
                Some(OsStr::new("/srv/perch")),
                Some(OsStr::new("/Users/alice"))
            )
            .unwrap(),
            PathBuf::from("/srv/perch")
        );
        assert_eq!(
            perch_home_in(Some(OsStr::new("~")), Some(OsStr::new("/Users/alice"))).unwrap(),
            PathBuf::from("/Users/alice")
        );
        assert_eq!(
            perch_home_in(
                Some(OsStr::new("~/custom/perch")),
                Some(OsStr::new("/Users/alice"))
            )
            .unwrap(),
            PathBuf::from("/Users/alice/custom/perch")
        );
        assert_eq!(
            perch_home_in(None, Some(OsStr::new("/Users/alice"))).unwrap(),
            PathBuf::from("/Users/alice/.perch")
        );
        assert_eq!(
            perch_home_in(Some(OsStr::new("")), Some(OsStr::new("/Users/alice"))).unwrap(),
            PathBuf::from("/Users/alice/.perch")
        );
        assert_eq!(
            perch_home_in(
                Some(OsStr::new("relative/perch")),
                Some(OsStr::new("/Users/alice"))
            ),
            Err("PERCH_HOME must be an absolute path".into())
        );
        assert_eq!(
            perch_home_in(Some(OsStr::new("~/perch")), None),
            Err("PERCH_HOME must be an absolute path".into())
        );
        assert_eq!(
            perch_home_in(None, None),
            Err("OpenCode requires HOME or PERCH_HOME".into())
        );
        assert_eq!(
            perch_home_in(None, Some(OsStr::new("relative"))),
            Err("OpenCode requires HOME or PERCH_HOME".into())
        );

        let dir = tempfile::tempdir().unwrap();
        let mut cmd = Command::new("opencode");
        configure_in(&mut cmd, dir.path(), false, &[], &[]).unwrap();
        assert!(dir.path().join("opencode/config").is_dir());
        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        let env_of = |key: &str| {
            envs.iter()
                .find(|(k, _)| *k == OsStr::new(key))
                .map(|(_, v)| *v)
        };
        assert_eq!(
            env_of("OPENCODE_DB"),
            Some(Some(dir.path().join("opencode/opencode.db").as_os_str()))
        );
        assert_eq!(
            env_of("OPENCODE_CONFIG_DIR"),
            Some(Some(dir.path().join("opencode/config").as_os_str()))
        );
        assert_eq!(env_of("OPENCODE_CONFIG"), Some(None));
        assert_eq!(
            env_of("XDG_DATA_HOME"),
            None,
            "XDG_DATA_HOME must not be overridden for child shell commands"
        );
    }

    #[test]
    fn an_archive_over_budget_does_not_disqualify_the_checkout_or_discussion() {
        let temp = tempfile::tempdir().unwrap();
        let checkout = temp.path().join("checkout");
        let discussion = temp.path().join("context");
        let archive = temp.path().join("archive");
        for dir in [&checkout, &discussion, &archive] {
            std::fs::create_dir(dir).unwrap();
            std::fs::write(dir.join("INDEX.md"), "").unwrap();
        }
        let existing = [discussion];
        let limit = 3;
        assert!(can_add_read_dir_with_budget(&checkout, &existing, &archive, limit));
        std::fs::write(archive.join("conversation.md"), "").unwrap();
        assert!(!can_add_read_dir_with_budget(&checkout, &existing, &archive, limit));
        assert!(refusal_with_budget(&checkout, &existing, limit).is_none());
    }

    #[test]
    fn trusted_work_checks_the_archive_independently() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        let archive = tmp.path().join("archive");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::create_dir(&archive).unwrap();
        let outside = tmp.path().join("instructions.md");
        std::fs::write(&outside, "").unwrap();
        std::os::unix::fs::symlink(&outside, checkout.join("AGENTS.md")).unwrap();
        let ask = crate::Mode::Ask;
        let work = crate::Mode::Work(crate::WorkGrant { trust: true, ..Default::default() });
        assert!(!crate::can_add_read_dir(Backend::OpenCode, &checkout, &[], &archive, &ask));
        assert!(crate::can_add_read_dir(Backend::OpenCode, &checkout, &[], &archive, &work));
        std::os::unix::fs::symlink(&outside, archive.join("AGENTS.md")).unwrap();
        assert!(!crate::can_add_read_dir(Backend::OpenCode, &checkout, &[], &archive, &work));
    }

    #[test]
    fn config_value_scopes_external_reads_and_protects_env_files() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        let snap_dir = real.join("snap");
        let diff_file = snap_dir.join("diff.patch");
        let sibling_file = snap_dir.join("sibling.txt");
        let discussion_dir = real.join("discussion");
        std::fs::create_dir_all(&snap_dir).unwrap();
        std::fs::create_dir_all(&discussion_dir).unwrap();
        std::fs::write(&diff_file, "diff").unwrap();
        std::fs::write(&sibling_file, "secret").unwrap();

        let external = vec![
            diff_file.clone(),
            discussion_dir.clone(),
            PathBuf::from("/"),
            real.join("wild*card"),
            real.join("wild?card"),
        ];

        for work in [false, true] {
            let gate = if work { "ask" } else { "deny" };
            let cfg = config_value(work, &external, &[]);
            let rules = cfg["agents"][mode(work)]["permissions"].as_array().unwrap();

            assert!(
                !rules.iter().any(|r| {
                    r["action"] == "external_directory"
                        && r["resource"] == "*"
                        && r["effect"] == "allow"
                }),
                "external_directory must never be blanket-allowed on *: {rules:?}"
            );

            let snap_glob = format!("{}/*", snap_dir.to_string_lossy());
            let diff_str = diff_file.to_string_lossy().into_owned();
            let sibling_str = sibling_file.to_string_lossy().into_owned();
            let disc_str = discussion_dir.to_string_lossy().into_owned();
            let disc_glob = format!("{disc_str}/*");

            assert!(rules.contains(&json!({
                "action": "external_directory",
                "resource": &snap_glob,
                "effect": "allow",
            })));
            assert!(rules.contains(&json!({
                "action": "read",
                "resource": &diff_str,
                "effect": "allow",
            })));
            assert!(!rules.contains(&json!({
                "action": "read",
                "resource": &snap_glob,
                "effect": "allow",
            })));

            assert!(rules.contains(&json!({
                "action": "external_directory",
                "resource": &disc_str,
                "effect": "allow",
            })));
            assert!(rules.contains(&json!({
                "action": "external_directory",
                "resource": &disc_glob,
                "effect": "allow",
            })));
            assert!(rules.contains(&json!({
                "action": "read",
                "resource": &disc_str,
                "effect": "allow",
            })));
            assert!(rules.contains(&json!({
                "action": "read",
                "resource": &disc_glob,
                "effect": "allow",
            })));

            let last_ext_allow = rules
                .iter()
                .rposition(|r| r["resource"] == disc_glob && r["effect"] == "allow")
                .unwrap();
            let first_env_rule = rules.iter().position(|r| r["resource"] == "*.env").unwrap();
            assert!(
                last_ext_allow < first_env_rule,
                ".env rules must follow external path allows so external .env files stay gated"
            );

            // Relative paths inside cwd: allowed except .env files.
            assert_eq!(eval_effect(rules, "read", "src/lib.rs"), "allow");
            assert_eq!(eval_effect(rules, "read", ".env"), gate);
            assert_eq!(eval_effect(rules, "read", "sub/.env.local"), gate);
            assert_eq!(eval_effect(rules, "read", ".env.example"), "allow");

            // External single file: diff.patch is readable, sibling in same dir is gated.
            assert_eq!(
                eval_effect(rules, "external_directory", &snap_glob),
                "allow"
            );
            assert_eq!(eval_effect(rules, "read", &diff_str), "allow");
            assert_eq!(eval_effect(rules, "read", &sibling_str), gate);

            // External directory: files inside are readable except .env files.
            assert_eq!(
                eval_effect(rules, "external_directory", &disc_glob),
                "allow"
            );
            assert_eq!(
                eval_effect(rules, "read", &format!("{disc_str}/INDEX.md")),
                "allow"
            );
            assert_eq!(
                eval_effect(rules, "read", &format!("{disc_str}/.env")),
                gate
            );

            // Unapproved external paths remain denied (or asked in work mode).
            assert_eq!(eval_effect(rules, "external_directory", "/etc/*"), gate);
            assert_eq!(eval_effect(rules, "read", "/etc/passwd"), gate);
            assert_eq!(eval_effect(rules, "read", "C:/Users/me/secret"), gate);

            for tool in ["edit", "shell", "webfetch", "websearch"] {
                assert_eq!(eval_effect(rules, tool, "anything"), gate);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn refusal_blocks_external_instruction_symlinks_unless_trusted() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "top-secret").unwrap();

        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        std::fs::write(repo.path().join("README.md"), "hello").unwrap();
        let sub = repo.path().join("sub");
        std::fs::create_dir(&sub).unwrap();

        // Regular AGENTS.md is allowed.
        let agents = sub.join("AGENTS.md");
        std::fs::write(&agents, "plain rules").unwrap();
        assert_eq!(refusal(repo.path(), &[], false), None);

        // Internal symlink staying inside the repository is allowed.
        std::fs::remove_file(&agents).unwrap();
        symlink("../README.md", &agents).unwrap();
        assert_eq!(refusal(repo.path(), &[], false), None);

        // External symlink escaping the repository is refused when untrusted, allowed when trusted.
        std::fs::remove_file(&agents).unwrap();
        symlink(&secret, &agents).unwrap();
        let why = refusal(repo.path(), &[], false).expect("external AGENTS.md symlink refused");
        assert!(why.contains("sub/AGENTS.md"), "{why}");
        assert!(why.contains("outside the repository"), "{why}");
        assert!(why.contains("OpenCode"), "{why}");
        assert!(why.contains("trust = true"), "{why}");
        assert_eq!(refusal(repo.path(), &[], true), None);

        // Case-insensitive CLAUDE.md symlink is also refused.
        std::fs::remove_file(&agents).unwrap();
        let claude = sub.join("claude.md");
        symlink(&secret, &claude).unwrap();
        let why = refusal(repo.path(), &[], false).expect("external claude.md symlink refused");
        assert!(why.contains("sub/claude.md"), "{why}");

        // Added external read directory with an escaping symlink is also refused unless trusted.
        std::fs::remove_file(&claude).unwrap();
        let extra = tempfile::tempdir().unwrap();
        symlink(&secret, extra.path().join("AGENTS.md")).unwrap();
        assert!(refusal(repo.path(), &[extra.path().to_path_buf()], false).is_some());
        assert_eq!(
            refusal(repo.path(), &[extra.path().to_path_buf()], true),
            None
        );
    }
}
