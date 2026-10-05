//! Send Feedback's background half: what Perch knows about itself, the
//! agent's look into it, the issues that look like it, and the send.
//! Nothing here reaches GitHub but `file` and `plus_one`, and each only
//! when its button is pressed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use jiff::Timestamp;

use super::{Ctx, Event};
use crate::feedback::{
    self, FeedbackContext, FeedbackKind, Notes, PrepareRequest, Prepared, Problem, Secrets,
    Section, SendError, Sent, SimilarIssue, Step, Subscribed, Subscription, REPO,
};

/// A command Diagnostics runs is given this long.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// The agent's look is given this long.
const DIAGNOSIS_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The run looking into a report, for Stop. One at a time: the draft is
/// one for the app.
static DIAGNOSIS: LazyLock<Mutex<Option<Arc<perch_agent::Run>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Collect, look into it, search, and put the preview together.
pub(super) async fn prepare(ctx: &Ctx, request: PrepareRequest) {
    let attempt = request.attempt;
    let step = |step| {
        let _ = ctx.events.send(Event::FeedbackStep { attempt, step });
    };
    step(Step::Collecting);
    let context = collect(ctx, &request).await;

    let (notes, notes_error) = match (&request.choice, request.diagnose) {
        (Some(choice), true) => {
            step(Step::Diagnosing);
            match diagnose(ctx, &request, &context, choice.clone()).await {
                Ok(notes) => (Some(notes), None),
                Err(why) => (None, Some(why)),
            }
        }
        _ => (None, None),
    };

    step(Step::Searching);
    let similar = similar_issues(ctx, &request.title).await;

    let text = feedback::render(
        request.kind,
        &request.body,
        request.diagnose,
        &context,
        notes.as_ref(),
    );
    let (preview, redacted) = feedback::redact(&text, &secrets(ctx));
    let _ = ctx.events.send(Event::FeedbackReady(Box::new(Prepared {
        attempt,
        context,
        notes,
        notes_error,
        preview,
        redacted,
        similar,
    })));
}

/// Stop the agent looking into the report, if one is.
pub(super) fn stop_diagnosis() {
    let run = DIAGNOSIS.lock().ok().and_then(|mut slot| slot.take());
    if let Some(run) = run {
        run.cancel();
    }
}

/// File the issue, or comment on one, with exactly the text given, after
/// one more redaction pass. A pass that changes anything sends nothing
/// and hands the redacted text back for the reader to check.
pub(super) async fn file(
    ctx: &Ctx,
    title: &str,
    body: &str,
    kind: FeedbackKind,
    comment_on: Option<u64>,
) {
    let failed = |error: SendError| {
        let _ = ctx.events.send(Event::FeedbackFailed(error));
    };
    let secrets = secrets(ctx);
    let (clean_title, title_caught) = feedback::redact(title, &secrets);
    let (clean_body, body_caught) = feedback::redact(body, &secrets);
    if title_caught + body_caught > 0 {
        let _ = ctx.events.send(Event::FeedbackCaught {
            title: clean_title,
            preview: clean_body,
            count: title_caught + body_caught,
        });
        return;
    }
    // The pattern pass can't know every token's shape; the one Perch holds
    // never goes, whatever.
    if let Some(session) = ctx.session().filter(|s| s.token.len() >= 20) {
        if feedback::holds(title, &session.token) || feedback::holds(body, &session.token) {
            failed(SendError {
                message: "The text still holds your GitHub token.".into(),
                forbidden: false,
            });
            return;
        }
    }
    let Some(github) = ctx.github() else {
        failed(offline());
        return;
    };
    let result = match comment_on {
        Some(number) => github
            .create_issue_comment(REPO, number, body)
            .await
            .map(|_| (number, Sent::Commented)),
        None => github
            .create_issue_labeled(REPO, title.trim(), body, &[kind.issue_label()])
            .await
            .map(|issue| (issue.number, Sent::Filed)),
    };
    match result {
        Ok((number, outcome)) => {
            let _ = ctx.events.send(Event::FeedbackSent { number, outcome });
        }
        Err(e) => failed(send_error(&e, "open issues on")),
    }
}

/// +1 a similar issue: the 👍 when `react`, then a subscription unless
/// the reader ignores it. A refused 👍 stops there; a refused subscribe
/// keeps the 👍 and says so.
pub(super) async fn plus_one(ctx: &Ctx, number: u64, node_id: &str, react: bool) {
    let Some(github) = ctx.github() else {
        let _ = ctx.events.send(Event::FeedbackFailed(offline()));
        return;
    };
    if react {
        if let Err(e) = github
            .set_reaction(node_id, perch_github::ReactionKind::ThumbsUp, true)
            .await
        {
            let _ = ctx
                .events
                .send(Event::FeedbackFailed(send_error(&e, "react to issues on")));
            return;
        }
    }
    // Read again: the reader may have ignored it since the preview.
    let now = github
        .issue_viewer_states(REPO, &[number])
        .await
        .ok()
        .and_then(|states| states.into_iter().next())
        .map(|state| Subscription::parse(&state.viewer_subscription));
    let subscription = match now {
        Some(Subscription::Ignored) => Subscribed::Ignored,
        Some(Subscription::Subscribed) => Subscribed::Yes,
        _ => match github.set_subscription(node_id, "SUBSCRIBED").await {
            Ok(state) if state == "SUBSCRIBED" => Subscribed::Yes,
            Ok(_) => Subscribed::Failed,
            Err(e) => {
                tracing::warn!("subscribing to {REPO}#{number}: {e}");
                Subscribed::Failed
            }
        },
    };
    let _ = ctx.events.send(Event::FeedbackSent {
        number,
        outcome: Sent::PlusOned {
            reacted: react,
            subscription,
        },
    });
}

fn offline() -> SendError {
    SendError {
        message: "Offline.".into(),
        forbidden: false,
    }
}

pub(super) fn send_error(e: &perch_github::Error, verb: &str) -> SendError {
    match e {
        perch_github::Error::Status {
            status: 403 | 404, ..
        } => SendError {
            message: format!("Your token can't {verb} {REPO}."),
            forbidden: true,
        },
        perch_github::Error::Network(_) => offline(),
        perch_github::Error::Unauthorized => SendError {
            message: "GitHub didn't accept your token.".into(),
            forbidden: false,
        },
        perch_github::Error::RateLimited { .. } => SendError {
            message: "GitHub's rate limit is in force. Try again in a few minutes.".into(),
            forbidden: false,
        },
        other => SendError {
            message: other.to_string(),
            forbidden: false,
        },
    }
}

/// Up to three open issues whose titles look like the report's, with
/// whether the reader gave each a 👍 and hears about it. Nothing on any
/// failure.
async fn similar_issues(ctx: &Ctx, title: &str) -> Vec<SimilarIssue> {
    let (Some(github), Some(query)) = (ctx.github(), feedback::search_query(title)) else {
        return Vec::new();
    };
    let hits = match github.search_issues(&query, 1, 3).await {
        Ok(page) => page.hits,
        Err(e) => {
            tracing::warn!("searching for similar issues: {e}");
            return Vec::new();
        }
    };
    let hits: Vec<_> = hits
        .into_iter()
        .filter(|hit| hit.is_open())
        .take(3)
        .collect();
    let numbers: Vec<u64> = hits.iter().map(|hit| hit.number).collect();
    let states = github
        .issue_viewer_states(REPO, &numbers)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("reading similar issues' reactions: {e}");
            Vec::new()
        });
    hits.into_iter()
        .filter_map(|hit| {
            let state = states.iter().find(|s| s.number == hit.number)?;
            Some(SimilarIssue {
                number: hit.number,
                title: hit.title,
                updated_at: Some(hit.updated_at),
                node_id: state.node_id.clone(),
                thumbs_up: state.thumbs_up,
                viewer_thumbs_up: state.viewer_thumbs_up,
                viewer_subscription: Subscription::parse(&state.viewer_subscription),
            })
        })
        .collect()
}

/// Ask the model menu's pick, under Ask's rules, why it happened.
async fn diagnose(
    ctx: &Ctx,
    request: &PrepareRequest,
    context: &FeedbackContext,
    (backend, model, effort): (perch_agent::Backend, String, perch_agent::Effort),
) -> Result<Notes, String> {
    let by = match backend {
        perch_agent::Backend::ClaudeCode => "Claude",
        perch_agent::Backend::Codex => "Codex",
    };
    let checkout = request
        .item_id
        .as_deref()
        .and_then(|id| ctx.item(id))
        .and_then(|item| item.checkout)
        .filter(|checkout| checkout.on_disk && checkout.path.is_dir())
        .map(|checkout| checkout.path);
    let home = perch_github::config::perch_home();
    let worktree = checkout.unwrap_or_else(|| home.clone());
    let prompt = crate::ask::Prompt {
        system: None,
        user: feedback::prompt(
            request.kind,
            &request.title,
            &request.body,
            context,
            &crate::log_file::dir().display().to_string(),
        ),
    };
    let spec = crate::ask::run_spec(
        &ctx.config().agent,
        backend,
        &model,
        effort,
        worktree,
        &prompt,
        None,
        perch_agent::Mode::Ask,
    );
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let run = match perch_agent::Run::spawn(spec, events) {
        Ok(run) => Arc::new(run),
        Err(e) => return Err(format!("{e}. {}", crate::ask::binary_hint(backend))),
    };
    if let Ok(mut slot) = DIAGNOSIS.lock() {
        if let Some(old) = slot.replace(run.clone()) {
            old.cancel();
        }
    }
    let mut said = String::new();
    let read = async {
        while let Some(event) = received.recv().await {
            if let perch_agent::RunEvent::Text(text) = event {
                if !text.trim().is_empty() {
                    said = text;
                }
            }
        }
    };
    let timed_out = tokio::time::timeout(DIAGNOSIS_TIMEOUT, read).await.is_err();
    if timed_out {
        run.cancel_and_wait().await;
    }
    let outcome = run.wait().await;
    if let Ok(mut slot) = DIAGNOSIS.lock() {
        if slot.as_ref().is_some_and(|r| Arc::ptr_eq(r, &run)) {
            slot.take();
        }
    }
    if timed_out {
        return Err(format!(
            "{by} took more than ten minutes, so it was stopped."
        ));
    }
    match outcome {
        perch_agent::RunOutcome::Finished { .. } if !said.trim().is_empty() => Ok(Notes {
            by: by.to_string(),
            text: said.trim().to_string(),
        }),
        perch_agent::RunOutcome::Finished { .. } => Err(format!("{by} wrote nothing.")),
        perch_agent::RunOutcome::Failed { message, .. } => Err(format!("{by} failed: {message}")),
        perch_agent::RunOutcome::Stopped { .. } => Err(format!("You stopped {by}.")),
    }
}

/// What redaction takes out on this machine.
fn secrets(ctx: &Ctx) -> Secrets {
    let mut exact: Vec<String> = std::env::vars()
        .filter(|(name, value)| perch_agent::is_scrubbed_env(name) && value.len() >= 6)
        .map(|(_, value)| value)
        .collect();
    if let Some(session) = ctx.session() {
        exact.push(session.token);
    }
    let config = ctx.config();
    if let Some(token) = config.token.clone() {
        exact.push(token);
    }
    let home = home();
    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| {
            home.as_ref()
                .and_then(|h| h.file_name())
                .map(|n| n.to_string_lossy().into_owned())
        });
    let mut repos: Vec<String> = config
        .repo
        .iter()
        .filter_map(|r| r.github.clone())
        .collect();
    let mut repo_paths: Vec<String> = config
        .repo
        .iter()
        .filter(|r| {
            !r.github
                .as_deref()
                .is_some_and(|g| g.eq_ignore_ascii_case(REPO))
        })
        .flat_map(|r| r.roots.iter().map(|root| expand(root, home.as_deref())))
        .collect();
    if let Some(items) = ctx.with_cache(|cache| cache.load_items().unwrap_or_default()) {
        for item in items {
            if item.repo_full_name.eq_ignore_ascii_case(REPO) {
                continue;
            }
            repos.push(item.repo_full_name.clone());
            if let Some(checkout) = item.checkout {
                repo_paths.push(checkout.path.display().to_string());
            }
        }
    }
    repos.sort();
    repos.dedup();
    repo_paths.sort();
    repo_paths.dedup();
    Secrets {
        exact,
        home: home.map(|h| h.display().to_string()),
        user,
        repos,
        repo_paths,
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn expand(path: &str, home: Option<&Path>) -> String {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest).display().to_string(),
        _ => path.to_string(),
    }
}

/// Everything Diagnostics says, before redaction.
async fn collect(ctx: &Ctx, request: &PrepareRequest) -> FeedbackContext {
    let config = ctx.config();
    let commit = env!("PERCH_COMMIT");
    let version = env!("CARGO_PKG_VERSION");

    let mut perch = vec![format!("Version: {version} ({commit})")];
    perch.push(format!(
        "Build: {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    ));
    perch.push(format!(
        "Launched from: {}",
        if std::os::unix::process::parent_id() == 1 {
            "Finder"
        } else {
            "a terminal"
        }
    ));

    let product = run(Path::new("sw_vers"), &["-productVersion"]).await;
    let build = run(Path::new("sw_vers"), &["-buildVersion"]).await;
    let macos = match (&product, &build) {
        (Ok(product), Ok(build)) => format!("{product} ({build})"),
        (Ok(product), Err(_)) => product.clone(),
        (Err(why), _) => why.clone(),
    };
    let arch = std::env::consts::ARCH;
    let mut system = vec![format!("macOS: {macos}"), format!("Architecture: {arch}")];
    system.extend(request.facts.display.iter().cloned());

    let agent = &config.agent;
    let claude = cli_line(
        "Claude Code",
        &agent.binary,
        agent.binary_version.as_deref(),
        !agent.models.is_empty(),
    );
    let codex = cli_line(
        "Codex",
        &agent.codex.binary,
        agent.codex.binary_version.as_deref(),
        !agent.codex.models.is_empty(),
    );
    let agents = vec![claude.0, codex.0];

    let servers = language_servers(ctx, request).await;

    let session = ctx.session();
    let mut github = vec![format!(
        "Hosts: {}",
        std::iter::once("github.com".to_string())
            .chain(config.github_host.clone())
            .collect::<Vec<_>>()
            .join(", ")
    )];
    github.push(format!(
        "Token from: {}",
        session
            .as_ref()
            .map(|s| s.source.as_str())
            .unwrap_or("none found")
    ));
    if let Some(poll) = &request.facts.last_poll {
        github.push(format!("Last poll: {poll}"));
    }
    github.push(format!(
        "Rate limit: {}",
        match session.as_ref().and_then(|s| s.github.rate_limited_until()) {
            Some(until) => format!(
                "in force until {}",
                clock(Timestamp::from_second(until).ok())
            ),
            None => "not in force".to_string(),
        }
    ));

    let config_lines = config_summary(&perch_github::config::perch_home().join("config.toml"));

    let mut item = Vec::new();
    match request.item_id.as_deref().and_then(|id| ctx.item(id)) {
        Some(open) => {
            let kind = match open.subject.as_ref().map(|s| s.kind()) {
                Some(perch_github::ItemKind::PullRequest) => "pull request",
                Some(perch_github::ItemKind::Issue) => "issue",
                None => "branch",
            };
            item.push(format!("Kind: {kind}"));
            item.push(format!(
                "Checkout: {}",
                match &open.checkout {
                    None => "none",
                    Some(c) if !c.on_disk => "removed",
                    Some(c) if c.owned => "Perch's own",
                    Some(_) => "under a root",
                }
            ));
            if let Some(status) = &request.facts.language_server {
                item.push(format!("Language server: {status}"));
            }
        }
        None => item.push("Nothing open".to_string()),
    }

    let problems = request
        .problems
        .iter()
        .map(|p| problem_line(p, request.seed.as_ref()))
        .collect();

    let log = crate::log_file::recent(&crate::log_file::dir(), feedback::LOG_LINES);

    let mut summary = format!(
        "Perch {version} ({commit}) · macOS {} {arch}",
        macos_short(&product, &build)
    );
    if let Some(v) = claude.1 {
        summary.push_str(&format!(" · Claude Code {v}"));
    }
    if let Some(v) = codex.1 {
        summary.push_str(&format!(" · Codex {v}"));
    }

    FeedbackContext {
        summary,
        sections: vec![
            Section {
                heading: "Perch".into(),
                lines: perch,
            },
            Section {
                heading: "System".into(),
                lines: system,
            },
            Section {
                heading: "Agents".into(),
                lines: agents,
            },
            Section {
                heading: "Language servers".into(),
                lines: servers,
            },
            Section {
                heading: "GitHub".into(),
                lines: github,
            },
            Section {
                heading: "Config".into(),
                lines: config_lines,
            },
            Section {
                heading: "The open item".into(),
                lines: item,
            },
            Section {
                heading: "Recent problems".into(),
                lines: problems,
            },
        ],
        log,
    }
}

fn macos_short(product: &Result<String, String>, build: &Result<String, String>) -> String {
    match (product, build) {
        (Ok(product), Ok(build)) => format!("{product} ({build})"),
        (Ok(product), Err(_)) => product.clone(),
        _ => "unknown".to_string(),
    }
}

fn problem_line(problem: &Problem, seed: Option<&Problem>) -> String {
    let tone = match problem.tone {
        crate::state::Tone::Error => "error",
        crate::state::Tone::Warning => "warning",
        crate::state::Tone::Success => "done",
    };
    let marked = if seed == Some(problem) {
        " (reported)"
    } else {
        ""
    };
    format!(
        "{} {tone}: {}{marked}",
        clock(Some(problem.at)),
        problem.text.lines().next().unwrap_or("")
    )
}

fn clock(at: Option<Timestamp>) -> String {
    at.map(|at| {
        at.to_zoned(jiff::tz::TimeZone::system())
            .strftime("%H:%M:%S")
            .to_string()
    })
    .unwrap_or_else(|| "?".to_string())
}

/// `Claude Code: /opt/homebrew/bin/claude, 2.1.230`, and the bare version
/// for the summary line.
fn cli_line(
    name: &str,
    binary: &str,
    version: Option<&str>,
    found: bool,
) -> (String, Option<String>) {
    if !found && version.is_none() {
        return (format!("{name}: not found"), None);
    }
    let short = version.and_then(version_number);
    let line = match version {
        Some(v) => format!("{name}: {binary}, {}", v.trim()),
        None => format!("{name}: {binary}"),
    };
    (line, short)
}

/// The first `1.2` or `1.2.3` in a `--version` line.
fn version_number(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|part| part.contains('.') && part.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|part| part.trim_end_matches('.').to_string())
}

/// The servers Perch runs, and every other one of their kinds on the
/// machine.
async fn language_servers(ctx: &Ctx, request: &PrepareRequest) -> Vec<String> {
    let mut clients: Vec<(&str, Arc<perch_lsp::Client>)> = Vec::new();
    let mut open: Vec<_> = ctx
        .language_servers
        .lock()
        .await
        .values()
        .cloned()
        .collect();
    open.sort_by_key(|client| client.kind());
    for client in open {
        clients.push(("Open checkout", client));
    }
    for client in ctx.kept_servers.clients() {
        clients.push(("Kept from an earlier item", client));
    }
    let processes = processes().await;
    let mut lines = Vec::new();
    if clients.is_empty() {
        lines.push(match &request.facts.language_server {
            Some(status) => format!("None running ({status})"),
            None => "None running".to_string(),
        });
    }
    let mut ours = Vec::new();
    for (role, client) in clients {
        let binary = client.binary();
        let kind = client.kind();
        lines.push(format!("{role}: {}", client.status().describe(kind)));
        lines.push(format!(
            "  Binary: {} (from {})",
            binary.display(),
            binary_source(binary)
        ));
        let version = run(binary, &["--version"]).await.unwrap_or_else(|why| why);
        lines.push(format!("  Version: {version}"));
        if kind == perch_lsp::ServerKind::RustAnalyzer {
            lines.push(format!("  Toolchain pin: {}", toolchain_pin(client.root())));
        }
        if let Some(pid) = client.pid() {
            ours.push(pid);
            let memory = processes
                .iter()
                .find(|p| p.pid == pid)
                .map(|p| megabytes(p.rss_kb))
                .unwrap_or_else(|| "gone".to_string());
            lines.push(format!(
                "  Process: pid {pid}, {memory}, up {}",
                duration(client.age())
            ));
        }
    }
    let mut others = Vec::new();
    for kind in perch_lsp::ServerKind::ALL {
        let found: Vec<&Process> = processes
            .iter()
            .filter(|p| p.command.ends_with(kind.name()) && !ours.contains(&p.pid))
            .collect();
        if !found.is_empty() {
            let total: u64 = found.iter().map(|p| p.rss_kb).sum();
            others.push(format!(
                "{} {}, {}",
                found.len(),
                kind.name(),
                megabytes(total)
            ));
        }
    }
    lines.push(if others.is_empty() {
        "Other language server processes: none".to_string()
    } else {
        format!("Other language server processes: {}", others.join("; "))
    });
    lines.push(format!(
        "Memory budget: {} (lsp.max_memory_mb)",
        megabytes(ctx.config().lsp.max_memory_mb.saturating_mul(1024))
    ));
    lines
}

fn binary_source(binary: &Path) -> &'static str {
    let text = binary.display().to_string();
    if text.contains("/.rustup/toolchains/") {
        "the checkout's toolchain"
    } else if text.contains("/.cargo/bin/") {
        "cargo's bin directory"
    } else {
        "PATH"
    }
}

/// The checkout's `rust-toolchain.toml` channel, or the file's word.
fn toolchain_pin(root: &Path) -> String {
    for name in ["rust-toolchain.toml", "rust-toolchain"] {
        let Ok(text) = std::fs::read_to_string(root.join(name)) else {
            continue;
        };
        let channel = text
            .lines()
            .map(str::trim)
            .find_map(|line| {
                line.strip_prefix("channel").map(|rest| {
                    rest.trim_start_matches([' ', '='])
                        .trim_matches('"')
                        .to_string()
                })
            })
            .or_else(|| (!text.contains('[')).then(|| text.trim().to_string()));
        return match channel {
            Some(channel) if !channel.is_empty() => format!("{channel} ({name})"),
            _ => format!("{name}, no channel"),
        };
    }
    "none".to_string()
}

struct Process {
    pid: u32,
    rss_kb: u64,
    command: String,
}

async fn processes() -> Vec<Process> {
    let Ok(out) = run(Path::new("ps"), &["-axo", "pid=,rss=,comm="]).await else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let rss_kb = parts.next()?.parse().ok()?;
            let command = parts.collect::<Vec<_>>().join(" ");
            Some(Process {
                pid,
                rss_kb,
                command,
            })
        })
        .collect()
}

fn megabytes(kb: u64) -> String {
    if kb >= 1024 * 1024 {
        format!("{:.1} GB", kb as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} MB", kb / 1024)
    }
}

fn duration(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
    }
}

/// A command's first line of output, or why there isn't one.
async fn run(program: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).kill_on_drop(true);
    match tokio::time::timeout(COMMAND_TIMEOUT, command.output()).await {
        Err(_) => Err("timed out".to_string()),
        Ok(Err(e)) => Err(format!("couldn't run: {e}")),
        Ok(Ok(out)) if !out.status.success() => Err(format!("failed ({})", out.status)),
        Ok(Ok(out)) => {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Ok(text)
        }
    }
}

/// The config file as Diagnostics lines: every key but `token`, flat,
/// and the `[[repo]]` list as what it holds rather than what it names.
fn config_summary(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec!["No config file".to_string()];
    };
    let table: toml::Table = match text.parse() {
        Ok(table) => table,
        Err(e) => return vec![format!("Unreadable: {}", e.message())],
    };
    let mut lines = Vec::new();
    for (key, value) in &table {
        match key.as_str() {
            "token" => lines.push("token: set (not shown)".to_string()),
            "repo" => lines.push(repo_summary(value)),
            _ => flatten(key, value, &mut lines),
        }
    }
    lines
}

fn flatten(prefix: &str, value: &toml::Value, lines: &mut Vec<String>) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                flatten(&format!("{prefix}.{key}"), value, lines);
            }
        }
        other => lines.push(format!("`{prefix} = {other}`")),
    }
}

fn repo_summary(value: &toml::Value) -> String {
    let Some(repos) = value.as_array() else {
        return "repo: not a list".to_string();
    };
    let tables: Vec<&toml::Table> = repos.iter().filter_map(|r| r.as_table()).collect();
    let on_github = tables.iter().filter(|t| t.contains_key("github")).count();
    let roots: usize = tables
        .iter()
        .map(|t| {
            t.get("roots")
                .and_then(|r| r.as_array())
                .map_or(0, Vec::len)
        })
        .sum();
    let trusted = tables
        .iter()
        .filter(|t| t.get("trust").and_then(|v| v.as_bool()) == Some(true))
        .count();
    let work = tables.iter().filter(|t| t.contains_key("work")).count();
    format!(
        "repo: {} entries, {on_github} on GitHub, {roots} roots in all, {trusted} with trust, {work} with [repo.work]",
        tables.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_come_out_of_version_lines() {
        assert_eq!(
            version_number("2.1.230 (Claude Code)").as_deref(),
            Some("2.1.230")
        );
        assert_eq!(
            version_number("codex-cli 0.160.0").as_deref(),
            Some("0.160.0")
        );
        assert_eq!(
            version_number("rust-analyzer 1.97.0 (abc 2026-08-01)").as_deref(),
            Some("1.97.0")
        );
        assert_eq!(version_number("none"), None);
    }

    #[test]
    fn the_config_loses_its_token_and_its_repositories_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
token = "ghp_secret"
[agent]
model = "sonnet"
[[repo]]
github = "acme/rocket"
roots = ["~/Code/rocket", "~/Code/rocket-2"]
trust = true
[[repo]]
roots = ["~/Code/side"]
[repo.work]
paths = ["/tmp"]
"#,
        )
        .unwrap();
        let lines = config_summary(&path).join("\n");
        assert!(!lines.contains("ghp_secret"), "{lines}");
        assert!(!lines.contains("acme"), "{lines}");
        assert!(!lines.contains("rocket"), "{lines}");
        assert!(lines.contains("token: set (not shown)"), "{lines}");
        assert!(lines.contains("`agent.model = \"sonnet\"`"), "{lines}");
        assert!(
            lines.contains(
                "repo: 2 entries, 1 on GitHub, 3 roots in all, 1 with trust, 1 with [repo.work]"
            ),
            "{lines}"
        );
    }

    #[test]
    fn a_toolchain_pin_names_its_channel() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(toolchain_pin(dir.path()), "none");
        std::fs::write(
            dir.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.97.0\"\n",
        )
        .unwrap();
        assert_eq!(toolchain_pin(dir.path()), "1.97.0 (rust-toolchain.toml)");
    }

    #[test]
    fn binaries_say_where_they_came_from() {
        assert_eq!(
            binary_source(Path::new(
                "/Users/a/.rustup/toolchains/1.97.0-aarch64-apple-darwin/bin/rust-analyzer"
            )),
            "the checkout's toolchain"
        );
        assert_eq!(
            binary_source(Path::new("/Users/a/.cargo/bin/rust-analyzer")),
            "cargo's bin directory"
        );
        assert_eq!(
            binary_source(Path::new("/opt/homebrew/bin/rust-analyzer")),
            "PATH"
        );
    }

    #[tokio::test]
    async fn a_command_that_hangs_times_out() {
        let started = std::time::Instant::now();
        let result = run(Path::new("sleep"), &["30"]).await;
        assert_eq!(result, Err("timed out".to_string()));
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
