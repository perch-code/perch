//! What a host lacks for its engine to do its work: a GitHub token, git, a
//! repository on the host, the agent CLIs, on Linux the tools Claude Code's
//! sandbox needs, and a logind that leaves the engine running when the ssh
//! session that started it ends. Only an engine under `perchd serve` looks,
//! at start and whenever its config is taken up again; the app's own engine
//! runs on the Mac the reader is sitting at.
//!
//! Each check says what to run to put it right. The token is only ever
//! looked for: whether there is one, and where it came from.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use perch_github::Config;

/// One thing the host has or lacks.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostCheck {
    /// What is looked for, as Settings names it: "git", "GitHub token".
    pub what: String,
    pub ok: bool,
    /// Whether the engine can't do its work without it. A required check
    /// that fails is what makes the app say the host needs setup.
    pub required: bool,
    /// What was found, or what is missing, in a sentence.
    pub detail: String,
    /// What to run on the host to put it right, when there is a command
    /// for it. `None` while it is fine.
    #[serde(default)]
    pub fix: Option<String>,
}

impl HostCheck {
    /// Whether this check stands between the host and its work.
    pub fn blocks(&self) -> bool {
        self.required && !self.ok
    }
}

/// Whether any required check failed: the host needs setup.
pub fn needs_setup(checks: &[HostCheck]) -> bool {
    checks.iter().any(HostCheck::blocks)
}

/// How long one look at the host may take, each command it runs and the
/// token's lookup alike. A check that runs out is taken as missing.
const LOOK: Duration = Duration::from_secs(10);

/// What a look at the host found, for [`checks`] to weigh.
#[derive(Debug, Clone, Default)]
pub(crate) struct Found {
    /// Where the GitHub token came from, `gh auth token` say, or `None`
    /// with none. Never the token.
    pub token: Option<&'static str>,
    /// The first line `git --version` printed, when git ran.
    pub git: Option<String>,
    /// Each `[[repo]]` with a root, by what the sidebar calls it, and
    /// whether the root is a directory on the host.
    pub repos: Vec<RepoRoot>,
    /// The engine's config.toml, for where to add a `[[repo]]`.
    pub config_path: Option<PathBuf>,
    /// The GitHub Enterprise host the config names, if any: a missing
    /// root is cloned from github.com only without one.
    pub github_host: Option<String>,
    /// Claude Code, Codex and agy, as discovery left them.
    pub agents: Vec<AgentFound>,
    /// On Linux, which of `bwrap` and `socat` are not on `PATH`. `None`
    /// off Linux, where Claude Code's sandbox is the system's.
    pub sandbox_missing: Option<Vec<&'static str>>,
    /// What logind does at logout, when it could be asked: Linux with
    /// `loginctl`.
    pub logind: Option<Logind>,
    /// The package manager on the host, for the commands that install.
    pub installer: Option<Installer>,
    /// Who the engine runs as.
    pub user: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoRoot {
    pub name: String,
    /// `owner/name`, or `local/<name>`, when known.
    pub identity: Option<String>,
    /// The root as config.toml has it, `~` and all.
    pub root: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agent {
    ClaudeCode,
    Codex,
    Gemini,
}

impl Agent {
    fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Gemini => "agy (Gemini)",
        }
    }

    /// The command that installs it, where there is one to give.
    fn install(self) -> Option<&'static str> {
        match self {
            Agent::ClaudeCode => Some("curl -fsSL https://claude.ai/install.sh | bash"),
            Agent::Codex => Some("npm install -g @openai/codex"),
            Agent::Gemini => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentFound {
    pub agent: Agent,
    /// The binary discovery looked for, as config.toml names it, or the
    /// path it was found at.
    pub binary: String,
    /// Where discovery found it, and what `--version` said.
    pub found: Option<(PathBuf, Option<String>)>,
}

/// What logind does with a user's processes at logout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Logind {
    /// Whether it kills them: never for a user in `KillExcludeUsers`;
    /// else, when `KillOnlyUsers` names any, for those it names, whatever
    /// `KillUserProcesses` says; else as `KillUserProcesses` says.
    pub kills: bool,
    /// What `KillUserProcesses` says, for the page.
    pub kill_user_processes: bool,
}

/// A package manager, and how it installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Installer {
    Apt,
    Dnf,
    Yum,
    Pacman,
    Zypper,
    Apk,
}

impl Installer {
    /// Each manager by the program that names it, in the order looked for.
    const ALL: [(&'static str, Installer); 6] = [
        ("apt-get", Installer::Apt),
        ("dnf", Installer::Dnf),
        ("yum", Installer::Yum),
        ("pacman", Installer::Pacman),
        ("zypper", Installer::Zypper),
        ("apk", Installer::Apk),
    ];

    /// The command that installs `packages`.
    pub(crate) fn command(self, packages: &[&str]) -> String {
        let packages = packages.join(" ");
        match self {
            Installer::Apt => format!("sudo apt-get install -y {packages}"),
            Installer::Dnf => format!("sudo dnf install -y {packages}"),
            Installer::Yum => format!("sudo yum install -y {packages}"),
            Installer::Pacman => format!("sudo pacman -S --needed {packages}"),
            Installer::Zypper => format!("sudo zypper install -y {packages}"),
            Installer::Apk => format!("sudo apk add {packages}"),
        }
    }

    /// The first manager whose program is in one of `dirs`.
    pub(crate) fn find(dirs: &[PathBuf]) -> Option<Installer> {
        Installer::ALL
            .into_iter()
            .find(|(program, _)| {
                dirs.iter()
                    .filter(|dir| !dir.as_os_str().is_empty())
                    .any(|dir| dir.join(program).is_file())
            })
            .map(|(_, installer)| installer)
    }
}

/// The command that installs `packages` with `installer`, or the words
/// for it with none known.
fn install(installer: Option<Installer>, packages: &[&str]) -> String {
    match installer {
        Some(installer) => installer.command(packages),
        None => format!(
            "Install {} with the host's package manager.",
            packages.join(" and ")
        ),
    }
}

/// What was found, weighed: each check in the order Settings lists them.
pub(crate) fn checks(found: &Found) -> Vec<HostCheck> {
    let mut checks = Vec::new();

    checks.push(match found.token {
        Some(source) => HostCheck {
            what: "GitHub token".into(),
            ok: true,
            required: true,
            detail: format!("From {source}."),
            fix: None,
        },
        None => HostCheck {
            what: "GitHub token".into(),
            ok: false,
            required: true,
            detail: "No GitHub token: GITHUB_TOKEN and GH_TOKEN are unset, `gh auth token` gives none and config.toml sets no token.".into(),
            fix: Some("gh auth login".into()),
        },
    });

    checks.push(match &found.git {
        Some(version) => HostCheck {
            what: "git".into(),
            ok: true,
            required: true,
            detail: format!("{version}."),
            fix: None,
        },
        None => HostCheck {
            what: "git".into(),
            ok: false,
            required: true,
            detail: "git is not on PATH.".into(),
            fix: Some(install(found.installer, &["git"])),
        },
    });

    checks.push(repositories(found));

    let any_agent = found.agents.iter().any(|a| a.found.is_some());
    for agent in &found.agents {
        checks.push(match &agent.found {
            Some((path, version)) => HostCheck {
                what: agent.agent.name().into(),
                ok: true,
                required: false,
                detail: match version {
                    Some(version) => format!("{}, {version}.", path.display()),
                    None => format!("{}.", path.display()),
                },
                fix: None,
            },
            None => HostCheck {
                what: agent.agent.name().into(),
                ok: false,
                // One agent is enough, Claude Code the one asked for when
                // there is none.
                required: !any_agent && agent.agent == Agent::ClaudeCode,
                detail: match (agent.agent, any_agent) {
                    (Agent::ClaudeCode, false) => format!(
                        "Not found as {}, and no other agent is installed, so no run can start.",
                        agent.binary
                    ),
                    (Agent::Gemini, _) => format!(
                        "Not found as {}. agy is the Antigravity CLI: antigravity.google/docs/cli.",
                        agent.binary
                    ),
                    _ => format!("Not found as {}.", agent.binary),
                },
                fix: agent.agent.install().map(str::to_string),
            },
        });
    }

    let claude = found
        .agents
        .iter()
        .any(|a| a.agent == Agent::ClaudeCode && a.found.is_some());
    if let (true, Some(missing)) = (claude, &found.sandbox_missing) {
        checks.push(if missing.is_empty() {
            HostCheck {
                what: "bubblewrap and socat".into(),
                ok: true,
                required: true,
                detail: "On PATH, for Claude Code's sandbox.".into(),
                fix: None,
            }
        } else {
            let packages: Vec<&str> = missing
                .iter()
                .map(|tool| if *tool == "bwrap" { "bubblewrap" } else { tool })
                .collect();
            HostCheck {
                what: "bubblewrap and socat".into(),
                ok: false,
                required: true,
                detail: format!(
                    "No {} on PATH. Claude Code's sandbox needs both on Linux, and its runs are refused without them.",
                    missing.join(" or ")
                ),
                fix: Some(install(found.installer, &packages)),
            }
        });
    }

    if let Some(logind) = &found.logind {
        let user = &found.user;
        checks.push(if logind.kills {
            HostCheck {
                what: "logind".into(),
                ok: false,
                required: true,
                detail: format!(
                    "{}: {user}'s processes are killed when the ssh session that started the engine ends, the engine and its runs with them.",
                    if logind.kill_user_processes {
                        "KillUserProcesses=yes".to_string()
                    } else {
                        format!("KillUserProcesses=no, but KillOnlyUsers names {user}")
                    }
                ),
                fix: Some(format!(
                    "sudo mkdir -p /etc/systemd/logind.conf.d && printf '[Login]\\nKillExcludeUsers=root {user}\\n' | sudo tee /etc/systemd/logind.conf.d/perch.conf && sudo systemctl restart systemd-logind"
                )),
            }
        } else {
            HostCheck {
                what: "logind".into(),
                ok: true,
                required: true,
                detail: if logind.kill_user_processes {
                    format!("KillUserProcesses=yes, but not for {user}: the engine outlives the ssh session.")
                } else {
                    "KillUserProcesses=no: the engine outlives the ssh session.".into()
                },
                fix: None,
            }
        });
    }
    checks
}

/// Whether a `[[repo]]` has its root on the host: the engine lists and
/// works on nothing without one.
fn repositories(found: &Found) -> HostCheck {
    let what = "Repositories".to_string();
    let here: Vec<&RepoRoot> = found.repos.iter().filter(|r| r.exists).collect();
    let gone: Vec<&RepoRoot> = found.repos.iter().filter(|r| !r.exists).collect();
    let file = found
        .config_path
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "config.toml".into());
    if !here.is_empty() {
        let names: Vec<&str> = here.iter().map(|r| r.name.as_str()).collect();
        let mut detail = format!("{} on the host.", names.join(", "));
        if !gone.is_empty() {
            let roots: Vec<&str> = gone.iter().map(|r| r.root.as_str()).collect();
            detail.push_str(&format!(" Not there: {}.", roots.join(", ")));
        }
        return HostCheck {
            what,
            ok: true,
            required: true,
            detail,
            fix: None,
        };
    }
    if gone.is_empty() {
        return HostCheck {
            what,
            ok: false,
            required: true,
            detail: format!(
                "No [[repo]] in {file} has a root. Add one whose root is a clone on the host."
            ),
            fix: None,
        };
    }
    let roots: Vec<&str> = gone.iter().map(|r| r.root.as_str()).collect();
    // A github.com repository can be cloned to where config.toml says.
    let clone = gone.iter().find_map(|r| {
        let identity = r.identity.as_deref()?;
        (found.github_host.is_none() && !identity.starts_with("local/")).then(|| {
            format!(
                "git clone https://github.com/{identity}.git {}",
                shell_word(&r.root)
            )
        })
    });
    HostCheck {
        what,
        ok: false,
        required: true,
        detail: format!(
            "No [[repo]] root in {file} is on the host: {}. Clone it there, or point root at a clone that is.",
            roots.join(", ")
        ),
        fix: clone,
    }
}

/// A root as one word to the shell, a leading `~/` left for it to expand.
fn shell_word(root: &str) -> String {
    match root.strip_prefix("~/") {
        Some(rest) => format!("~/{}", crate::wire::remote::shell_quote(rest)),
        None => crate::wire::remote::shell_quote(root),
    }
}

/// Read what `loginctl show-session -p KillUserProcesses -p KillOnlyUsers
/// -p KillExcludeUsers` printed, the manager's settings, for `user`. As
/// logind decides it: never for a user it excludes; else, with
/// `KillOnlyUsers` set, for those it names and no others, whatever
/// `KillUserProcesses` says; else as `KillUserProcesses` says. logind
/// excludes root when `KillExcludeUsers` is not set, and `loginctl` prints
/// that; when it prints nothing of it, root is taken as excluded. `None`
/// when the output does not say.
pub(crate) fn logind_said(shown: &str, user: &str) -> Option<Logind> {
    let mut kill = None;
    let mut only: Vec<&str> = Vec::new();
    let mut exclude: Vec<&str> = vec!["root"];
    for line in shown.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        match key {
            "KillUserProcesses" => kill = Some(matches!(value.trim(), "yes" | "true" | "1")),
            "KillOnlyUsers" => only = value.split_whitespace().collect(),
            "KillExcludeUsers" => exclude = value.split_whitespace().collect(),
            _ => {}
        }
    }
    let kill_user_processes = kill?;
    let kills = if exclude.contains(&user) {
        false
    } else if !only.is_empty() {
        only.contains(&user)
    } else {
        kill_user_processes
    };
    Some(Logind {
        kills,
        kill_user_processes,
    })
}

/// Run `program` with `args` and give what it printed on stdout, when it
/// ran and exited 0 within [`LOOK`].
async fn output(program: &OsStr, args: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(LOOK, command.output())
        .await
        .ok()?
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The first line `git --version` prints, with `git` the program to run.
pub(crate) async fn git_version(git: &OsStr) -> Option<String> {
    let said = output(git, &["--version"]).await?;
    let line = said.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// What logind does at logout for `user`, asked of `loginctl`. `None`
/// with no `loginctl`, or no logind for it to ask, as in a container.
pub(crate) async fn ask_logind(loginctl: &OsStr, user: &str) -> Option<Logind> {
    let shown = output(
        loginctl,
        &[
            "show-session",
            "-p",
            "KillUserProcesses",
            "-p",
            "KillOnlyUsers",
            "-p",
            "KillExcludeUsers",
        ],
    )
    .await?;
    logind_said(&shown, user)
}

/// Look at the host for what `config`, whose agents discovery has found,
/// needs of it. `token` says where the token came from, or that there is
/// none, as the engine resolves it: it may run `gh`, so it runs off the
/// runtime's threads.
pub async fn look(
    config: &Config,
    token: impl FnOnce() -> Option<&'static str> + Send + 'static,
) -> Vec<HostCheck> {
    let linux = cfg!(target_os = "linux");
    let path = std::env::var_os("PATH");
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default();
    let token = async {
        tokio::time::timeout(LOOK, tokio::task::spawn_blocking(token))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
    };
    let logind = async {
        if linux && !user.is_empty() {
            ask_logind(OsStr::new("loginctl"), &user).await
        } else {
            None
        }
    };
    let on_disk = {
        let config = config.clone();
        let path = path.clone();
        tokio::task::spawn_blocking(move || on_disk(&config, path.as_deref(), linux))
    };
    let (token, git, logind, on_disk) = tokio::join!(
        token,
        git_version(OsStr::new("git")),
        logind,
        tokio::time::timeout(LOOK, on_disk)
    );
    let mut found = on_disk.ok().and_then(Result::ok).unwrap_or_default();
    found.token = token;
    found.git = git;
    found.logind = logind;
    found.user = user;
    found.config_path = perch_github::config::config_path();
    found.github_host = config.github_host.clone().filter(|h| !h.is_empty());
    checks(&found)
}

/// What a look finds on disk: the roots, the agents, the sandbox's tools
/// and the package manager. Directory lookups only, run off the runtime.
fn on_disk(config: &Config, path: Option<&OsStr>, linux: bool) -> Found {
    let dirs: Vec<PathBuf> = path.into_iter().flat_map(std::env::split_paths).collect();
    let repos = config
        .repo
        .iter()
        .filter_map(|r| {
            let root = r.root.clone()?;
            let exists = r.root_path().is_some_and(|p| p.is_dir());
            let name = r
                .name
                .clone()
                .or_else(|| r.identity().map(str::to_string))
                .unwrap_or_else(|| root.clone());
            Some(RepoRoot {
                name,
                identity: r.identity().map(str::to_string),
                root,
                exists,
            })
        })
        .collect();
    let agent = &config.agent;
    let agents = [
        (Agent::ClaudeCode, &agent.binary, &agent.binary_version),
        (
            Agent::Codex,
            &agent.codex.binary,
            &agent.codex.binary_version,
        ),
        (
            Agent::Gemini,
            &agent.gemini.binary,
            &agent.gemini.binary_version,
        ),
    ]
    .into_iter()
    .map(|(which, binary, version)| AgentFound {
        agent: which,
        binary: binary.clone(),
        // Discovery has put the path it found in place of the name; one
        // it did not find is still the name, and is not found again here.
        found: Path::new(binary)
            .is_absolute()
            .then(|| perch_agent::find_binary(binary))
            .flatten()
            .map(|found| (found, version.clone())),
    })
    .collect();
    let mut installer_dirs = dirs.clone();
    installer_dirs.extend(["/usr/bin", "/usr/sbin", "/bin", "/sbin"].map(PathBuf::from));
    Found {
        repos,
        agents,
        sandbox_missing: linux.then(|| perch_agent::linux_sandbox_missing(path)),
        installer: Installer::find(&installer_dirs),
        ..Found::default()
    }
}

/// The checks an engine has looked at last, and the look under way, so a
/// slow look never lands over one started after it.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    generation: AtomicU64,
    last: std::sync::Mutex<Option<Vec<HostCheck>>>,
}

impl Watch {
    /// A new look begins: what it lands under.
    pub(crate) fn begin(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Keep what the look `generation` found, unless another has begun
    /// since. Hands back the checks to send when they are new, so a config
    /// taken up with nothing changed for the host says nothing.
    pub(crate) fn land(&self, generation: u64, checks: Vec<HostCheck>) -> Option<Vec<HostCheck>> {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.generation.load(Ordering::SeqCst) != generation {
            return None;
        }
        if last.as_ref() == Some(&checks) {
            return None;
        }
        *last = Some(checks.clone());
        Some(checks)
    }

    /// The checks looked at last, for a client that asks for everything.
    pub(crate) fn last(&self) -> Option<Vec<HostCheck>> {
        self.last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `PATH` as a list.
    fn path_of(dirs: &[&Path]) -> std::ffi::OsString {
        std::env::join_paths(dirs).unwrap()
    }

    fn agent(agent: Agent, found: bool) -> AgentFound {
        AgentFound {
            agent,
            binary: match agent {
                Agent::ClaudeCode => "claude",
                Agent::Codex => "codex",
                Agent::Gemini => "agy",
            }
            .into(),
            found: found.then(|| (PathBuf::from("/usr/local/bin/x"), Some("1.0".into()))),
        }
    }

    fn all_there() -> Found {
        Found {
            token: Some("gh auth token"),
            git: Some("git version 2.43.0".into()),
            repos: vec![RepoRoot {
                name: "perch".into(),
                identity: Some("perch-code/perch".into()),
                root: "~/code/perch".into(),
                exists: true,
            }],
            config_path: Some("/home/chris/.perch/config.toml".into()),
            github_host: None,
            agents: vec![
                agent(Agent::ClaudeCode, true),
                agent(Agent::Codex, true),
                agent(Agent::Gemini, false),
            ],
            sandbox_missing: Some(vec![]),
            logind: Some(Logind {
                kills: false,
                kill_user_processes: false,
            }),
            installer: Some(Installer::Apt),
            user: "chris".into(),
        }
    }

    fn named<'a>(checks: &'a [HostCheck], what: &str) -> &'a HostCheck {
        checks
            .iter()
            .find(|c| c.what == what)
            .unwrap_or_else(|| panic!("no {what} in {checks:?}"))
    }

    #[test]
    fn a_host_with_everything_needs_no_setup() {
        let checks = checks(&all_there());
        assert!(!needs_setup(&checks), "{checks:#?}");
        let whats: Vec<&str> = checks.iter().map(|c| c.what.as_str()).collect();
        assert_eq!(
            whats,
            [
                "GitHub token",
                "git",
                "Repositories",
                "Claude Code",
                "Codex",
                "agy (Gemini)",
                "bubblewrap and socat",
                "logind"
            ]
        );
        assert_eq!(named(&checks, "GitHub token").detail, "From gh auth token.");
        // A missing agy is said, with nothing to install it by, and is
        // not required beside the others.
        let agy = named(&checks, "agy (Gemini)");
        assert!(!agy.ok && !agy.required && agy.fix.is_none());
        assert!(checks.iter().filter(|c| c.ok).all(|c| c.fix.is_none()));
    }

    #[test]
    fn each_missing_thing_says_what_to_run() {
        let found = Found {
            token: None,
            git: None,
            sandbox_missing: Some(vec!["bwrap", "socat"]),
            logind: Some(Logind {
                kills: true,
                kill_user_processes: true,
            }),
            installer: Some(Installer::Dnf),
            ..all_there()
        };
        let checks = checks(&found);
        assert!(needs_setup(&checks));
        let token = named(&checks, "GitHub token");
        assert!(token.blocks());
        assert_eq!(token.fix.as_deref(), Some("gh auth login"));
        assert_eq!(
            named(&checks, "git").fix.as_deref(),
            Some("sudo dnf install -y git")
        );
        let sandbox = named(&checks, "bubblewrap and socat");
        assert!(sandbox.blocks());
        assert!(sandbox.detail.starts_with("No bwrap or socat on PATH"));
        assert_eq!(
            sandbox.fix.as_deref(),
            Some("sudo dnf install -y bubblewrap socat")
        );
        let logind = named(&checks, "logind");
        assert!(logind.blocks());
        let fix = logind.fix.as_deref().unwrap();
        assert!(fix.contains("KillExcludeUsers=root chris"), "{fix}");
        assert!(fix.ends_with("sudo systemctl restart systemd-logind"));
    }

    #[test]
    fn the_sandbox_is_checked_only_on_linux_with_claude_code() {
        let found = Found {
            sandbox_missing: Some(vec!["socat"]),
            ..all_there()
        };
        let sandbox = checks(&found)
            .into_iter()
            .find(|c| c.what == "bubblewrap and socat")
            .unwrap();
        assert_eq!(
            sandbox.fix.as_deref(),
            Some("sudo apt-get install -y socat")
        );
        // Off Linux, no check.
        let mac = Found {
            sandbox_missing: None,
            logind: None,
            ..all_there()
        };
        let checks_off_linux = checks(&mac);
        assert!(!checks_off_linux
            .iter()
            .any(|c| c.what == "bubblewrap and socat" || c.what == "logind"));
        // Without Claude Code, no check: nothing needs the sandbox.
        let mut no_claude = found.clone();
        no_claude.agents[0] = agent(Agent::ClaudeCode, false);
        let checks = checks(&no_claude);
        assert!(!checks.iter().any(|c| c.what == "bubblewrap and socat"));
        // Codex is there, so a missing Claude Code is not required.
        assert!(!named(&checks, "Claude Code").required);
        assert_eq!(
            named(&checks, "Claude Code").fix.as_deref(),
            Some("curl -fsSL https://claude.ai/install.sh | bash")
        );
    }

    #[test]
    fn with_no_agent_at_all_claude_code_is_required() {
        let found = Found {
            agents: vec![
                agent(Agent::ClaudeCode, false),
                agent(Agent::Codex, false),
                agent(Agent::Gemini, false),
            ],
            ..all_there()
        };
        let checks = checks(&found);
        let claude = named(&checks, "Claude Code");
        assert!(claude.blocks());
        assert!(claude.detail.contains("no other agent is installed"));
        assert!(!named(&checks, "Codex").required);
        assert_eq!(
            named(&checks, "Codex").fix.as_deref(),
            Some("npm install -g @openai/codex")
        );
    }

    #[test]
    fn a_repository_root_has_to_be_on_the_host() {
        let gone = |identity: &str, root: &str| RepoRoot {
            name: "x".into(),
            identity: Some(identity.into()),
            root: root.into(),
            exists: false,
        };
        // None there: the first github.com one can be cloned where it goes.
        let found = Found {
            repos: vec![
                gone("local/notes", "~/notes"),
                gone("perch-code/perch", "~/code/my perch"),
            ],
            ..all_there()
        };
        let repos = repositories(&found);
        assert!(repos.blocks());
        assert!(repos.detail.contains("~/notes, ~/code/my perch"));
        assert_eq!(
            repos.fix.as_deref(),
            Some("git clone https://github.com/perch-code/perch.git ~/'code/my perch'")
        );
        // On GitHub Enterprise, which host is not known: no command.
        let enterprise = Found {
            github_host: Some("ghe.example.com".into()),
            ..found.clone()
        };
        assert_eq!(repositories(&enterprise).fix, None);
        // No [[repo]] with a root at all.
        let none = Found {
            repos: vec![],
            ..all_there()
        };
        let repos = repositories(&none);
        assert!(repos.blocks() && repos.fix.is_none());
        assert!(repos.detail.contains("/home/chris/.perch/config.toml"));
        // One there is enough; the others are named.
        let some = Found {
            repos: vec![all_there().repos[0].clone(), gone("a/b", "/srv/b")],
            ..all_there()
        };
        let repos = repositories(&some);
        assert!(repos.ok);
        assert_eq!(repos.detail, "perch on the host. Not there: /srv/b.");
    }

    #[test]
    fn logind_kills_as_logind_decides() {
        let said = |text: &str| logind_said(text, "chris");
        assert_eq!(
            said("KillUserProcesses=no\nKillOnlyUsers=\nKillExcludeUsers=root\n"),
            Some(Logind {
                kills: false,
                kill_user_processes: false
            })
        );
        assert!(
            said("KillUserProcesses=yes\nKillOnlyUsers=\nKillExcludeUsers=root\n")
                .unwrap()
                .kills
        );
        // Excluded, or not among the only ones killed.
        let excluded = said("KillUserProcesses=yes\nKillExcludeUsers=root chris\n").unwrap();
        assert!(!excluded.kills && excluded.kill_user_processes);
        assert!(
            !said("KillUserProcesses=yes\nKillOnlyUsers=alice bob\n")
                .unwrap()
                .kills
        );
        assert!(
            said("KillUserProcesses=yes\nKillOnlyUsers=alice chris\n")
                .unwrap()
                .kills
        );
        // KillOnlyUsers decides by itself, whatever KillUserProcesses says.
        let only =
            said("KillUserProcesses=no\nKillOnlyUsers=chris\nKillExcludeUsers=root\n").unwrap();
        assert!(only.kills && !only.kill_user_processes);
        assert!(
            !said("KillUserProcesses=no\nKillOnlyUsers=alice\nKillExcludeUsers=root\n")
                .unwrap()
                .kills
        );
        // Excluded wins over KillOnlyUsers.
        assert!(
            !said("KillUserProcesses=yes\nKillOnlyUsers=chris\nKillExcludeUsers=chris\n")
                .unwrap()
                .kills
        );
        // Root is excluded unless KillExcludeUsers says otherwise.
        let root = |text: &str| logind_said(text, "root").unwrap().kills;
        assert!(!root("KillUserProcesses=yes\n"));
        assert!(!root("KillUserProcesses=yes\nKillExcludeUsers=root\n"));
        assert!(root("KillUserProcesses=yes\nKillExcludeUsers=\n"));
        let killed = checks(&Found {
            logind: Some(only),
            ..all_there()
        });
        let killed = named(&killed, "logind");
        assert!(killed.blocks());
        assert!(
            killed
                .detail
                .starts_with("KillUserProcesses=no, but KillOnlyUsers names chris: "),
            "{}",
            killed.detail
        );
        // Nothing said of it: no check.
        assert_eq!(said("Docked=no\n"), None);
        assert_eq!(said(""), None);

        let ok = checks(&Found {
            logind: Some(excluded),
            ..all_there()
        });
        assert_eq!(
            named(&ok, "logind").detail,
            "KillUserProcesses=yes, but not for chris: the engine outlives the ssh session."
        );
    }

    #[test]
    fn a_package_manager_is_found_on_path_in_order() {
        let none = tempfile::tempdir().unwrap();
        let both = tempfile::tempdir().unwrap();
        for program in ["dnf", "apt-get"] {
            std::fs::write(both.path().join(program), "").unwrap();
        }
        assert_eq!(Installer::find(&[none.path().to_path_buf()]), None);
        assert_eq!(
            Installer::find(&[none.path().to_path_buf(), both.path().to_path_buf()]),
            Some(Installer::Apt)
        );
        assert_eq!(
            Installer::Pacman.command(&["git"]),
            "sudo pacman -S --needed git"
        );
        assert_eq!(Installer::Apk.command(&["git"]), "sudo apk add git");
        assert_eq!(
            install(None, &["bubblewrap", "socat"]),
            "Install bubblewrap and socat with the host's package manager."
        );
    }

    #[test]
    fn the_watch_keeps_the_latest_look_and_says_only_news() {
        let watch = Watch::default();
        let first = watch.begin();
        let second = watch.begin();
        let checks = checks(&all_there());
        // A slow first look lands after the second began: dropped.
        assert_eq!(watch.land(first, checks.clone()), None);
        assert_eq!(watch.last(), None);
        assert_eq!(watch.land(second, checks.clone()), Some(checks.clone()));
        // The same again says nothing; a change does.
        let third = watch.begin();
        assert_eq!(watch.land(third, checks.clone()), None);
        let fourth = watch.begin();
        let fewer = checks[..2].to_vec();
        assert_eq!(watch.land(fourth, fewer.clone()), Some(fewer.clone()));
        assert_eq!(watch.last(), Some(fewer));
    }

    #[test]
    fn a_check_reads_as_it_did_before_fix_was_added() {
        let check: HostCheck = serde_json::from_str(
            r#"{"what":"git","ok":true,"required":true,"detail":"git version 2.43.0."}"#,
        )
        .unwrap();
        assert_eq!(check.fix, None);
        let line = serde_json::to_string(&checks(&all_there())).unwrap();
        let back: Vec<HostCheck> = serde_json::from_str(&line).unwrap();
        assert_eq!(back, checks(&all_there()));
    }

    /// A fake program in a temporary directory, printing `stdout`.
    #[cfg(unix)]
    fn fake(dir: &Path, name: &str, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn loginctl_and_git_are_asked_and_tolerated_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let loginctl = fake(
            dir.path(),
            "loginctl",
            r#"[ "$1" = show-session ] || exit 1
printf 'KillUserProcesses=yes\nKillOnlyUsers=\nKillExcludeUsers=root\n'"#,
        );
        assert_eq!(
            ask_logind(loginctl.as_os_str(), "chris").await,
            Some(Logind {
                kills: true,
                kill_user_processes: true
            })
        );
        // No logind to ask, as in a container: no answer, no check.
        let failing = fake(
            dir.path(),
            "loginctl-down",
            "echo 'Failed to connect to bus' >&2; exit 1",
        );
        assert_eq!(ask_logind(failing.as_os_str(), "chris").await, None);
        let gone = dir.path().join("nowhere");
        assert_eq!(ask_logind(gone.as_os_str(), "chris").await, None);

        let git = fake(dir.path(), "git", "echo 'git version 2.43.0'");
        assert_eq!(
            git_version(git.as_os_str()).await.as_deref(),
            Some("git version 2.43.0")
        );
        assert_eq!(git_version(gone.as_os_str()).await, None);
    }

    #[test]
    fn on_disk_takes_the_agents_discovery_found_and_the_roots() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("claude");
        std::fs::write(&claude, "").unwrap();
        let mut config = Config::parse_text(&format!(
            "[[repo]]\ngithub = \"a/b\"\nroot = \"{}\"\n\n[[repo]]\ngithub = \"c/d\"\nroot = \"{}\"\n",
            dir.path().display(),
            dir.path().join("gone").display()
        ))
        .unwrap();
        config.agent.binary = claude.display().to_string();
        config.agent.binary_version = Some("2.0.0 (Claude Code)".into());
        // Not found by discovery: still the name, never looked for again.
        config.agent.codex.binary = "codex".into();
        config.agent.gemini.binary = "agy".into();
        let found = on_disk(&config, Some(&path_of(&[dir.path()])), true);
        assert_eq!(
            found.agents[0].found,
            Some((claude.clone(), Some("2.0.0 (Claude Code)".into())))
        );
        assert_eq!(found.agents[1].found, None);
        assert_eq!(found.agents[2].found, None);
        assert_eq!(
            found.repos.iter().map(|r| r.exists).collect::<Vec<_>>(),
            [true, false]
        );
        assert_eq!(found.sandbox_missing, Some(vec!["bwrap", "socat"]));
        assert_eq!(on_disk(&config, None, false).sandbox_missing, None);
    }
}
