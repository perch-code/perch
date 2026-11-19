//! config.toml as the Settings page edits it. A write goes through
//! `perch_github::write_settings`, which changes only the keys it is given,
//! and then the whole file is taken up again, as it is after an edit made
//! by hand while Perch runs: the file stays the one place settings live.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use perch_git::cmd::Git;
use perch_github::{AgentConfig, Config};

use super::{
    detect_review_capabilities, hold_untrusted_servers, scan_roots, start_held_servers_everywhere,
    Ctx, Event,
};

/// What the app checks in a config as it is read, beyond what the engine
/// knows: the keys `[keybindings]` names are the window's. What it finds
/// goes in the config's warnings and is said as the file's others are.
/// Given to [`spawn`](super::spawn), so no way of starting the engine
/// can leave it out.
pub type CheckConfig = fn(&mut Config);

/// config.toml as the Settings page shows it, sent at startup, after every
/// write and after an edit made outside Perch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SettingsFile {
    pub path: PathBuf,
    /// The file's settings as written: the agents' binaries as typed, not
    /// as discovery found them. Each `[[repo]]` carries the identity its
    /// root resolved to, which is how a setting names it. `None` when the
    /// file doesn't parse.
    pub config: Option<Arc<Config>>,
    /// Why the file can't be read or doesn't parse. Perch goes on with the
    /// settings it read last.
    pub error: Option<String>,
    /// The keys the file sets, in dotted form, such as `display.zoom` or
    /// `repo.owner/name.work.trust`: the ones a reset can take out.
    pub present: BTreeSet<String>,
    /// Everything loading the file had to say.
    pub warnings: Vec<String>,
    /// Why each `[[knowledge]]` directory that runs are not given isn't,
    /// by its `dir`: missing, a symlink or an agent's instructions file in
    /// it, or too many files to check. Checked on the engine's machine.
    #[serde(default)]
    pub knowledge_problems: BTreeMap<PathBuf, String>,
}

impl SettingsFile {
    pub(super) fn read(path: &Path, text: &str, config: &Config) -> Self {
        SettingsFile {
            path: path.to_path_buf(),
            present: perch_github::present_keys(text, &config.repo),
            warnings: config.warnings.clone(),
            config: Some(Arc::new(config.clone())),
            error: None,
            knowledge_problems: BTreeMap::new(),
        }
    }

    pub(super) fn broken(path: &Path, error: String) -> Self {
        SettingsFile {
            path: path.to_path_buf(),
            config: None,
            error: Some(error),
            present: BTreeSet::new(),
            warnings: Vec::new(),
            knowledge_problems: BTreeMap::new(),
        }
    }
}

/// How often config.toml is looked at for edits made outside Perch.
pub(super) const CONFIG_CHECK: Duration = Duration::from_secs(2);

/// When the file last changed, as far as a cheap look can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

/// The file's stamp, or `None` when there is no file to stamp.
pub(super) fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

/// The file's text, empty when there is no file, and the config it loads
/// as, or why it can't be read or loaded.
pub(super) fn read(path: &Path) -> Result<(String, Config), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("can't read {}: {e}", path.display())),
    };
    let config = Config::parse_text(&text)?;
    Ok((text, config))
}

/// What the backend keeps of config.toml between reads.
pub(super) struct Held {
    /// Taken for each take-up and each write, so a write and the poll that
    /// notices it never interleave.
    turn: tokio::sync::Mutex<()>,
    /// The file as it was when last read. A take-up records it, so the
    /// poll doesn't take up Perch's own write a second time.
    stamp: std::sync::Mutex<Option<Stamp>>,
    /// `[agent]` as the file had it when last read, before discovery
    /// turned names into paths and asked the CLIs for their models. A
    /// change to a binary or a model list here is what runs discovery
    /// again.
    file_agent: std::sync::RwLock<AgentConfig>,
    /// The warnings the last read gave, so a take-up shows only new ones.
    warnings: std::sync::Mutex<HashSet<String>>,
    /// The last Settings sent, sent again when a write is refused.
    last: std::sync::Mutex<Option<SettingsFile>>,
    /// Counts the discoveries started, so a slow one doesn't land over one
    /// started after it.
    discovery: AtomicU64,
    /// Told when a take-up changes `poll_interval_secs`, so the command
    /// loop moves the poll it has scheduled rather than waiting it out.
    poll_interval: tokio::sync::Notify,
    /// The app's check of each read, as startup ran it.
    check: CheckConfig,
}

impl Default for Held {
    fn default() -> Self {
        Held::new(None, AgentConfig::default(), &[], None, |_| {})
    }
}

impl Held {
    /// What startup read: the file's stamp from before the read, its
    /// `[agent]`, the warnings already shown, and the Settings sent; and
    /// the app's check, for every read after it.
    pub(super) fn new(
        stamp: Option<Stamp>,
        file_agent: AgentConfig,
        warnings: &[String],
        sent: Option<SettingsFile>,
        check: CheckConfig,
    ) -> Self {
        Held {
            turn: tokio::sync::Mutex::new(()),
            stamp: std::sync::Mutex::new(stamp),
            file_agent: std::sync::RwLock::new(file_agent),
            warnings: std::sync::Mutex::new(warnings.iter().cloned().collect()),
            last: std::sync::Mutex::new(sent),
            discovery: AtomicU64::new(0),
            poll_interval: tokio::sync::Notify::new(),
            check,
        }
    }

    /// Ready once a take-up has changed the poll interval since the last
    /// time it was.
    pub(super) fn poll_interval_changed(&self) -> tokio::sync::futures::Notified<'_> {
        self.poll_interval.notified()
    }

    fn stamp(&self) -> Option<Stamp> {
        *self
            .stamp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_stamp(&self, stamp: Option<Stamp>) {
        *self
            .stamp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stamp;
    }

    fn file_agent(&self) -> AgentConfig {
        self.file_agent
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Put the file's new `[agent]` in place. Says whether a binary or a
    /// model list changed, which discovery has to look at again.
    fn swap_file_agent(&self, agent: &AgentConfig) -> bool {
        let mut held = self
            .file_agent
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = needs_discovery(&held, agent);
        *held = agent.clone();
        changed
    }

    /// The warnings in `all` the last read didn't give. They become the
    /// ones seen, so a warning fixed and then made again is shown again.
    fn new_warnings(&self, all: &[String]) -> Vec<String> {
        let mut seen = self
            .warnings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fresh = all.iter().filter(|w| !seen.contains(*w)).cloned().collect();
        *seen = all.iter().cloned().collect();
        fresh
    }

    /// Held while the Settings go, so a client's resync never sends an
    /// older file after this one.
    fn send(&self, ctx: &Ctx, mut file: SettingsFile) {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Until the sources are checked again, what was found last stands.
        if let Some(previous) = last.as_ref() {
            file.knowledge_problems = previous.knowledge_problems.clone();
        }
        *last = Some(file.clone());
        let _ = ctx.events.send(Event::Settings(file));
    }

    /// Put the knowledge sources' problems on the Settings last sent, and
    /// send them again when that changes what they say.
    pub(super) fn set_knowledge_problems(&self, ctx: &Ctx, problems: BTreeMap<PathBuf, String>) {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(file) = last.as_mut() else {
            return;
        };
        if file.knowledge_problems != problems {
            file.knowledge_problems = problems;
            let _ = ctx.events.send(Event::Settings(file.clone()));
        }
    }

    /// Run `send` with the Settings last sent, for a client that asks for
    /// them again, and no write's Settings sent meanwhile.
    pub(super) fn with_last<T>(&self, send: impl FnOnce(Option<SettingsFile>) -> T) -> T {
        let last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        send(last.clone())
    }

    fn send_again(&self, ctx: &Ctx) {
        let last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(file) = last {
            let _ = ctx.events.send(Event::Settings(file));
        }
    }
}

/// Whether discovery has to run again between two of the file's `[agent]`
/// tables: a binary or a model list changed. The model, effort, run
/// limit and sandbox are patched onto what discovery found instead.
fn needs_discovery(before: &AgentConfig, after: &AgentConfig) -> bool {
    before.binary != after.binary
        || before.models != after.models
        || before.codex.binary != after.codex.binary
        || before.codex.models != after.codex.models
        || before.gemini.binary != after.gemini.binary
        || before.gemini.models != after.gemini.models
        || before.opencode.binary != after.opencode.binary
        || before.opencode.models != after.opencode.models
}

/// What discovery found, with the file's own model, effort, run limit
/// and sandbox.
fn patched(discovered: &AgentConfig, file: &AgentConfig) -> AgentConfig {
    let mut agent = discovered.clone();
    agent.model = file.model.clone();
    agent.effort = file.effort.clone();
    agent.max_concurrent_runs = file.max_concurrent_runs;
    agent.sandbox = file.sandbox;
    crate::ask::settle_model(&mut agent);
    agent
}

/// Make `changes` to the file at `path` and take the result up. A refused
/// write is said, and the page is sent the file as it was, so a control
/// that showed the new value goes back.
pub(super) async fn write(ctx: &Ctx, path: &Path, changes: Vec<perch_github::Setting>) {
    let known = ctx.config().repo.clone();
    let written = {
        let _turn = ctx.settings.turn.lock().await;
        perch_github::write_settings(path, &changes, &known)
    };
    match written {
        Ok(_) => {
            let _ = take_up_whole_config(ctx, path).await;
        }
        Err(message) => {
            ctx.notice(message);
            ctx.settings.send_again(ctx);
        }
    }
}

/// Look at the file again, and take it up when it has changed since it was
/// last read. A file that has gone is left alone: an editor may be between
/// taking the old one away and putting the new one down, and Perch keeps
/// what it read last. Says whether it took the file up.
pub(super) async fn look_again(ctx: &Ctx, path: &Path) -> bool {
    let now = stamp(path);
    if now.is_none() || now == ctx.settings.stamp() {
        return false;
    }
    let _ = take_up_whole_config(ctx, path).await;
    true
}

/// Look at the file every [`CONFIG_CHECK`] for as long as Perch runs.
pub(super) fn watch(ctx: Ctx, path: PathBuf) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(CONFIG_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick is now, and startup has just read the file.
        tick.tick().await;
        loop {
            tick.tick().await;
            look_again(&ctx, &path).await;
        }
    });
}

/// Read the whole file again and put it in place of the running config.
///
/// A file that doesn't parse changes nothing: the page is told why, and
/// Perch goes on with what it read last. Otherwise the repositories are
/// resolved as at startup and the new config swapped in whole, except for
/// what discovery found about the agents: their paths, versions and
/// catalog models stay while the file's binaries and model lists stay the
/// same, and discovery runs again when they don't. Every window then hears
/// the new repositories, approvals, defaults, hosts and agents, and a
/// warning not seen before goes up as a notice. A root added, moved or
/// given another base is scanned straight away rather than at the next
/// poll.
///
/// Returns the running config the file now gives.
pub(super) async fn take_up_whole_config(ctx: &Ctx, path: &Path) -> Result<Config, String> {
    let held = ctx.settings.clone();
    let _turn = held.turn.lock().await;
    // Before the read, so an edit landing during it is looked at again.
    held.set_stamp(stamp(path));
    let (text, mut file) = match read(path) {
        Ok(read) => read,
        Err(error) => {
            held.send(ctx, SettingsFile::broken(path, error.clone()));
            return Err(error);
        }
    };
    crate::repos::resolve(&mut file, &Git::new(None)).await;
    (held.check)(&mut file);

    let before = ctx.config();
    let rediscover = held.swap_file_agent(&file.agent);
    let mut running = file.clone();
    running.agent = patched(&before.agent, &file.agent);
    ctx.replace_config(running.clone());

    if running.poll_interval_secs != before.poll_interval_secs {
        held.poll_interval.notify_one();
    }
    if running.agent.max_concurrent_runs != before.agent.max_concurrent_runs {
        ctx.runs.set_limit(running.agent.max_concurrent_runs);
    }
    let _ = ctx.events.send(Event::Repos(running.declared_repos()));
    ctx.send_work_approvals();
    let _ = ctx.events.send(Event::WorkDefaults(running.work.clone()));
    let _ = ctx.events.send(Event::GithubHosts(running.github_hosts()));
    if running.agent != before.agent {
        let _ = ctx.events.send(Event::AgentConfig(running.agent.clone()));
    }
    for warning in held.new_warnings(&running.warnings) {
        ctx.notice(warning);
    }
    held.send(ctx, SettingsFile::read(path, &text, &file));
    tokio::spawn(super::knowledge::refresh_knowledge_status(ctx.clone()));

    // Servers held for code a repository now trusts may start; ones on
    // show for code a repository no longer trusts go back to asking.
    let untrusted = before.repo.iter().any(|r| {
        r.lsp.trust
            && r.identity()
                .is_some_and(|identity| !running.lsp_trusted(identity))
    });
    // A root's identity is read off its remotes on the hosts, and its
    // branches with it; a root added, moved or given another base has
    // branches and worktrees no scan has listed yet.
    let rescan =
        before.github_hosts() != running.github_hosts() || scanned(&before) != scanned(&running);
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            start_held_servers_everywhere(&ctx).await;
            if untrusted {
                for kind in perch_lsp::ServerKind::ALL {
                    hold_untrusted_servers(&ctx, kind).await;
                }
            }
            if rescan && scan_roots(&ctx).await {
                ctx.send_items();
            }
        });
    }
    if rediscover {
        discover_again(ctx, file.agent.clone());
    } else {
        ctx.look_at_host();
    }
    Ok(running)
}

/// What a scan of the roots reads off the config: each repository's
/// identity, root and base.
fn scanned(config: &Config) -> BTreeSet<(Option<String>, Option<PathBuf>, Option<String>)> {
    config
        .repo
        .iter()
        .map(|r| {
            (
                r.identity().map(str::to_lowercase),
                r.root_path(),
                r.base.clone(),
            )
        })
        .collect()
}

/// Find the agents the file now names, as startup does, and put what is
/// found in place unless a newer discovery has started meanwhile. The
/// file's model, effort and run limit are read when it lands, since they
/// may have changed while it ran.
fn discover_again(ctx: &Ctx, mut agent: AgentConfig) {
    let generation = ctx.settings.discovery.fetch_add(1, Ordering::SeqCst) + 1;
    let ctx = ctx.clone();
    tokio::spawn(async move {
        for problem in crate::ask::discover(&mut agent).await {
            tracing::warn!(%problem, "agent discovery");
        }
        let capabilities = detect_review_capabilities(&agent).await;
        let held = ctx.settings.clone();
        let _turn = held.turn.lock().await;
        if held.discovery.load(Ordering::SeqCst) != generation {
            return;
        }
        let agent = patched(&agent, &held.file_agent());
        tracing::info!(
            claude = %agent.binary,
            codex = ?agent.codex.models,
            gemini = ?agent.gemini.models,
            opencode = ?agent.opencode.models,
            "agents found again"
        );
        if agent.models.is_empty()
            && agent.codex.models.is_empty()
            && agent.gemini.models.is_empty()
            && agent.opencode.models.is_empty()
        {
            ctx.notice(
                "No coding agent found. Install Claude Code, Codex, agy (Gemini) or OpenCode, or set agent.binary in config.toml.",
            );
        }
        ctx.replace_agent(agent.clone());
        ctx.set_review_capabilities(capabilities);
        let _ = ctx.events.send(Event::AgentConfig(agent));
        ctx.look_at_host();
    });
}
