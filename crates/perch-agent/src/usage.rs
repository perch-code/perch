//! How much of their plan's limits the reader has used, per CLI.
//!
//! Three sources, one board. `claude -p /usage` prints Claude Code's
//! limits as data without calling the model; `codex app-server` answers
//! `account/rateLimits/read` without starting a thread. Both are asked on
//! a timer. And every run Perch starts says the same as it goes: Claude's
//! stream carries a `rate_limit_event`, Codex's app-server an
//! `account/rateLimits/updated`. The parser hands each of those lines here,
//! so the board is as fresh as the last run or the last poll, whichever
//! came later.
//!
//! The board is the process's, not a run's: the limits are the account's,
//! and every run of one CLI reports the same account.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use jiff::Timestamp;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{watch, Notify};

use crate::Backend;

/// One CLI's limits as it last reported them.
#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    /// "Pro", "Plus". Codex says; Claude's report does not.
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    /// The CLI says work is refused until a window resets.
    pub limited: bool,
    /// Claude is billing work past the plan's limits as extra usage.
    pub overage: bool,
    /// Codex's free rate-limit resets the account holds.
    pub free_resets: Option<u64>,
    /// When Perch heard it.
    pub observed_at: Timestamp,
}

/// One rolling limit.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// The same window whichever source reported it: `session`, `weekly`,
    /// `weekly:Fable`.
    pub key: String,
    /// "5-hour", "Weekly", "Weekly · Fable".
    pub label: String,
    /// 0 to 100, and past 100 when a CLI says so.
    pub used_percent: f64,
    pub resets_at: Option<Timestamp>,
}

/// What is known of one CLI's limits.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Account {
    pub usage: Option<Usage>,
    /// Why the last poll found nothing, until one succeeds. `usage`, if
    /// there is any, is older than that poll.
    pub error: Option<String>,
}

/// Every CLI's limits.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Board {
    pub claude: Account,
    pub codex: Account,
    /// agy reports no limits Perch can read; this stays empty.
    pub gemini: Account,
}

impl Board {
    pub fn account(&self, backend: Backend) -> &Account {
        match backend {
            Backend::ClaudeCode => &self.claude,
            Backend::Codex => &self.codex,
            Backend::Gemini => &self.gemini,
        }
    }

    fn account_mut(&mut self, backend: Backend) -> &mut Account {
        match backend {
            Backend::ClaudeCode => &mut self.claude,
            Backend::Codex => &mut self.codex,
            Backend::Gemini => &mut self.gemini,
        }
    }
}

fn board() -> &'static watch::Sender<Board> {
    static BOARD: OnceLock<watch::Sender<Board>> = OnceLock::new();
    BOARD.get_or_init(|| watch::channel(Board::default()).0)
}

/// The board as it is now, and every change to it after.
pub fn subscribe() -> watch::Receiver<Board> {
    board().subscribe()
}

/// Put what a CLI reported on the board. A poll is the whole picture and
/// replaces what was there; a run's event may name only some windows, and
/// the others stand.
pub fn record(backend: Backend, usage: Usage, whole: bool) {
    board().send_modify(|board| {
        let account = board.account_mut(backend);
        account.usage = Some(match account.usage.take() {
            Some(old) if !whole => merge(old, usage),
            _ => usage,
        });
        if whole {
            account.error = None;
        }
    });
}

fn record_error(backend: Backend, error: String) {
    board().send_modify(|board| board.account_mut(backend).error = Some(error));
}

/// `new` over `old`: its windows replace theirs by key, and what it does
/// not say, the plan and the free resets, is kept.
fn merge(old: Usage, new: Usage) -> Usage {
    let mut windows = old.windows;
    for window in new.windows {
        match windows.iter_mut().find(|w| w.key == window.key) {
            Some(slot) => *slot = window,
            None => windows.push(window),
        }
    }
    Usage {
        plan: new.plan.or(old.plan),
        windows,
        limited: new.limited,
        overage: new.overage,
        free_resets: new.free_resets.or(old.free_resets),
        observed_at: new.observed_at,
    }
}

/// A line of a run's output, in case it reports limits.
pub(crate) fn observe(backend: Backend, line: &Value) {
    let usage = match backend {
        Backend::ClaudeCode => from_claude_event(line),
        Backend::Codex => (line.get("method").and_then(Value::as_str)
            == Some("account/rateLimits/updated"))
        .then(|| line.pointer("/params/rateLimits"))
        .flatten()
        .and_then(from_codex_snapshot),
        Backend::Gemini => None,
    };
    if let Some(usage) = usage {
        record(backend, usage, false);
    }
}

/// Claude's `rate_limit_event`: each unified window's utilization, 0 to 1,
/// and when it resets.
pub fn from_claude_event(line: &Value) -> Option<Usage> {
    if line.get("type").and_then(Value::as_str) != Some("rate_limit_event") {
        return None;
    }
    let info = line.get("rate_limit_info")?;
    let mut windows: Vec<Window> = info
        .get("unifiedWindows")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(kind, w)| {
            let used = w.get("utilization").and_then(Value::as_f64)?;
            let (key, label) = claude_event_window(kind);
            Some(Window {
                key,
                label,
                used_percent: used * 100.0,
                resets_at: w.get("resetsAt").and_then(Value::as_i64).and_then(epoch),
            })
        })
        .collect();
    // An older event names only the window it is about.
    if windows.is_empty() {
        let kind = info.get("rateLimitType").and_then(Value::as_str)?;
        let used = info.get("utilization").and_then(Value::as_f64)?;
        let (key, label) = claude_event_window(kind);
        windows.push(Window {
            key,
            label,
            used_percent: used * 100.0,
            resets_at: info.get("resetsAt").and_then(Value::as_i64).and_then(epoch),
        });
    }
    order(&mut windows);
    Some(Usage {
        plan: None,
        windows,
        limited: info.get("status").and_then(Value::as_str) == Some("rejected"),
        overage: info.get("isUsingOverage").and_then(Value::as_bool) == Some(true),
        free_resets: None,
        observed_at: Timestamp::now(),
    })
}

/// `five_hour`, `seven_day` and `seven_day_<model>` in an event's words.
fn claude_event_window(kind: &str) -> (String, String) {
    match kind {
        "five_hour" => ("session".into(), "5-hour".into()),
        "seven_day" => ("weekly".into(), "Weekly".into()),
        other => match other.strip_prefix("seven_day_") {
            Some(model) => scoped(&capitalize(model)),
            None => (other.into(), capitalize(&other.replace('_', " "))),
        },
    }
}

fn scoped(model: &str) -> (String, String) {
    (format!("weekly:{model}"), format!("Weekly · {model}"))
}

/// The `/usage` command's report, on the line of `claude -p /usage
/// --output-format stream-json` that carries `usage_report`.
pub fn from_claude_report(line: &Value) -> Option<Usage> {
    let limits = line
        .pointer("/usage_report/rate_limits/limits")?
        .as_array()?;
    let mut windows: Vec<Window> = limits
        .iter()
        .filter_map(|limit| {
            let used = limit.get("percent").and_then(Value::as_f64)?;
            let kind = limit.get("kind").and_then(Value::as_str).unwrap_or("");
            let (key, label) = match kind {
                "session" => ("session".into(), "5-hour".into()),
                "weekly_all" => ("weekly".into(), "Weekly".into()),
                _ => match limit
                    .pointer("/scope/model/display_name")
                    .and_then(Value::as_str)
                {
                    Some(model) => scoped(model),
                    None => (kind.into(), capitalize(&kind.replace('_', " "))),
                },
            };
            Some(Window {
                key,
                label,
                used_percent: used,
                resets_at: limit
                    .get("resets_at")
                    .and_then(Value::as_str)
                    .and_then(|at| at.parse().ok()),
            })
        })
        .collect();
    order(&mut windows);
    Some(Usage {
        plan: None,
        limited: windows.iter().any(|w| w.used_percent >= 100.0),
        windows,
        overage: false,
        free_resets: None,
        observed_at: Timestamp::now(),
    })
}

/// A Codex `RateLimitSnapshot`, as `account/rateLimits/read` and
/// `account/rateLimits/updated` carry it. Only the `codex` limit: a
/// model with its own quota reports under another id.
pub fn from_codex_snapshot(snapshot: &Value) -> Option<Usage> {
    match snapshot.get("limitId").and_then(Value::as_str) {
        None | Some("codex") => {}
        Some(_) => return None,
    }
    let mut windows: Vec<Window> = ["primary", "secondary"]
        .iter()
        .filter_map(|slot| {
            let w = snapshot.get(*slot)?;
            let used = w.get("usedPercent").and_then(Value::as_f64)?;
            let (key, label) = codex_window(w.get("windowDurationMins").and_then(Value::as_u64));
            Some(Window {
                key,
                label,
                used_percent: used,
                resets_at: w.get("resetsAt").and_then(Value::as_i64).and_then(epoch),
            })
        })
        .collect();
    order(&mut windows);
    Some(Usage {
        plan: snapshot
            .get("planType")
            .and_then(Value::as_str)
            .map(capitalize),
        windows,
        limited: snapshot
            .get("rateLimitReachedType")
            .is_some_and(|reached| !reached.is_null()),
        overage: false,
        free_resets: None,
        observed_at: Timestamp::now(),
    })
}

/// The whole answer to `account/rateLimits/read`: the snapshot, and the
/// free resets beside it.
pub fn from_codex_read(result: &Value) -> Option<Usage> {
    let snapshot = result
        .pointer("/rateLimitsByLimitId/codex")
        .or_else(|| result.get("rateLimits"))?;
    let mut usage = from_codex_snapshot(snapshot)?;
    usage.free_resets = result
        .pointer("/rateLimitResetCredits/availableCount")
        .and_then(Value::as_u64);
    if result.get("ordinaryUsageAllowed").and_then(Value::as_bool) == Some(false) {
        usage.limited = true;
    }
    Some(usage)
}

/// A window named by its length, which is all Codex says of it.
fn codex_window(minutes: Option<u64>) -> (String, String) {
    match minutes {
        Some(300) => ("session".into(), "5-hour".into()),
        Some(10080) => ("weekly".into(), "Weekly".into()),
        Some(m) if m % 1440 == 0 => (format!("{m}m"), format!("{}-day", m / 1440)),
        Some(m) if m % 60 == 0 => (format!("{m}m"), format!("{}-hour", m / 60)),
        Some(m) => (format!("{m}m"), format!("{m}-minute")),
        None => ("window".into(), "Limit".into()),
    }
}

/// Shortest window first, a model's own after the ones for every model.
fn order(windows: &mut [Window]) {
    let rank = |w: &Window| match w.key.as_str() {
        "session" => 0,
        "weekly" => 2,
        k if k.starts_with("weekly:") => 3,
        _ => 1,
    };
    windows.sort_by_key(rank);
}

fn epoch(seconds: i64) -> Option<Timestamp> {
    Timestamp::from_second(seconds).ok()
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// How long a poll may take before it is given up on.
const POLL_TIMEOUT: Duration = Duration::from_secs(30);

/// Ask Claude Code for `/usage`. A local command: no model is called and
/// nothing is billed. Run with no settings, hooks or MCP servers, since
/// none of them bear on the answer and some would slow it down.
pub async fn read_claude(binary: &Path, cwd: &Path) -> Result<Usage, String> {
    let mut command = Command::new(binary);
    command
        .args([
            "-p",
            "/usage",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            r#"{"mcpServers":{}}"#,
        ])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::scrub_env(&mut command);
    let output = tokio::time::timeout(POLL_TIMEOUT, command.output())
        .await
        .map_err(|_| "Claude Code took too long to report usage".to_string())?
        .map_err(|e| format!("could not run {}: {e}", binary.display()))?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|line| from_claude_report(&line))
        .ok_or_else(|| "Claude Code reported no plan usage".to_string())
}

/// Ask Codex's app-server for the account's limits: initialize, one
/// read, and close stdin, which ends the server.
pub async fn read_codex(binary: &Path, cwd: &Path) -> Result<Usage, String> {
    let mut command = Command::new(binary);
    command
        .arg("app-server")
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::scrub_env(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", binary.display()))?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err("Codex's app-server has no pipes".into());
    };
    let exchange = async move {
        let mut lines = BufReader::new(stdout).lines();
        let send = |message: Value| {
            let mut line = message.to_string();
            line.push('\n');
            line
        };
        let initialize = send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {"name": "perch", "version": env!("CARGO_PKG_VERSION")}},
        }));
        stdin
            .write_all(initialize.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("method").is_some() {
                continue;
            }
            if let Some(error) = v.pointer("/error/message").and_then(Value::as_str) {
                return Err(error.to_string());
            }
            match v.get("id").and_then(Value::as_u64) {
                Some(1) => {
                    let ask = send(json!({"jsonrpc": "2.0", "method": "initialized"}))
                        + &send(json!({
                            "jsonrpc": "2.0",
                            "id": 2,
                            "method": "account/rateLimits/read",
                        }));
                    stdin
                        .write_all(ask.as_bytes())
                        .await
                        .map_err(|e| e.to_string())?;
                }
                Some(2) => {
                    return v
                        .get("result")
                        .and_then(from_codex_read)
                        .ok_or_else(|| "Codex reported no plan usage".to_string());
                }
                _ => {}
            }
        }
        Err("Codex's app-server ended without an answer".to_string())
    };
    let answer = tokio::time::timeout(POLL_TIMEOUT, exchange)
        .await
        .unwrap_or_else(|_| Err("Codex took too long to report usage".to_string()));
    let _ = child.kill().await;
    answer
}

/// The CLIs to poll, and where to run them.
#[derive(Debug, Clone)]
pub struct Poll {
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
    pub cwd: PathBuf,
    pub every: Duration,
}

/// How soon after one poll `refresh_soon` may start another.
const REFRESH_SPACING: Duration = Duration::from_secs(60);

fn wake() -> &'static Arc<Notify> {
    static WAKE: OnceLock<Arc<Notify>> = OnceLock::new();
    WAKE.get_or_init(|| Arc::new(Notify::new()))
}

/// Poll every CLI in `poll` now and then every `poll.every`, until the
/// runtime ends. Must be called on a tokio runtime.
pub fn keep_fresh(poll: Poll) {
    let last = Arc::new(Mutex::new(None::<Instant>));
    tokio::spawn(async move {
        loop {
            *last.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
            let claude = async {
                if let Some(binary) = &poll.claude {
                    report(Backend::ClaudeCode, read_claude(binary, &poll.cwd).await);
                }
            };
            let codex = async {
                if let Some(binary) = &poll.codex {
                    report(Backend::Codex, read_codex(binary, &poll.cwd).await);
                }
            };
            tokio::join!(claude, codex);
            // A wake while the poll ran is not lost: `Notify` keeps one.
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(poll.every) => break,
                    _ = wake().notified() => {
                        let recent = last
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .is_some_and(|at| at.elapsed() < REFRESH_SPACING);
                        if !recent {
                            break;
                        }
                    }
                }
            }
        }
    });
}

/// Poll again now, unless a poll started in the last minute.
pub fn refresh_soon() {
    wake().notify_one();
}

fn report(backend: Backend, read: Result<Usage, String>) {
    match read {
        Ok(usage) => record(backend, usage, true),
        Err(error) => {
            tracing::warn!(backend = backend.name(), %error, "could not read plan usage");
            record_error(backend, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_EVENT: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1790553600,"rateLimitType":"seven_day","utilization":0.95,"isUsingOverage":false,"surpassedThreshold":0.75,"unifiedWindows":{"five_hour":{"utilization":0.02,"resetsAt":1790546400},"seven_day":{"utilization":0.95,"resetsAt":1790553600}}},"uuid":"7ba9cd85","session_id":"c5e62aac"}"#;

    const CLAUDE_REPORT: &str = r#"{"type":"assistant","usage_report":{"rate_limits":{"limits":[{"kind":"session","group":"session","percent":3,"resets_at":"2026-09-27T22:00:00.081948+00:00","scope":null,"severity":"normal","is_active":false},{"kind":"weekly_all","group":"weekly","percent":95,"resets_at":"2026-09-28T00:00:00.081970+00:00","scope":null,"severity":"critical","is_active":true},{"kind":"weekly_scoped","group":"weekly","percent":16,"resets_at":"2026-09-28T00:00:00.082141+00:00","scope":{"model":{"display_name":"Fable"},"surface":null},"severity":"normal","is_active":false}],"extra_usage":{"is_enabled":false}}}}"#;

    const CODEX_READ: &str = r#"{"ordinaryUsageAllowed":false,"rateLimits":{"limitId":"codex","primary":{"usedPercent":100,"windowDurationMins":10080,"resetsAt":1791046709},"secondary":null,"planType":"pro","rateLimitReachedType":"rate_limit_reached"},"rateLimitsByLimitId":{"codex":{"limitId":"codex","primary":{"usedPercent":100,"windowDurationMins":10080,"resetsAt":1791046709},"secondary":null,"planType":"pro","rateLimitReachedType":"rate_limit_reached"}},"rateLimitResetCredits":{"availableCount":1,"credits":[]}}"#;

    fn v(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    fn keys(usage: &Usage) -> Vec<(&str, &str, f64)> {
        usage
            .windows
            .iter()
            .map(|w| (w.key.as_str(), w.label.as_str(), w.used_percent.round()))
            .collect()
    }

    #[test]
    fn a_claude_event_names_its_unified_windows() {
        let usage = from_claude_event(&v(CLAUDE_EVENT)).unwrap();
        assert_eq!(
            keys(&usage),
            [("session", "5-hour", 2.0), ("weekly", "Weekly", 95.0)]
        );
        assert_eq!(
            usage.windows[0].resets_at,
            Some(Timestamp::from_second(1790546400).unwrap())
        );
        assert!(!usage.limited);
        assert!(from_claude_event(&v(r#"{"type":"assistant"}"#)).is_none());
    }

    #[test]
    fn a_rejected_claude_event_is_limited_and_an_old_one_names_one_window() {
        let usage = from_claude_event(&v(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","utilization":1.0,"resetsAt":1790546400,"isUsingOverage":true}}"#,
        ))
        .unwrap();
        assert_eq!(keys(&usage), [("session", "5-hour", 100.0)]);
        assert!(usage.limited);
        assert!(usage.overage);
    }

    #[test]
    fn claudes_usage_report_has_the_models_own_window() {
        let usage = from_claude_report(&v(CLAUDE_REPORT)).unwrap();
        assert_eq!(
            keys(&usage),
            [
                ("session", "5-hour", 3.0),
                ("weekly", "Weekly", 95.0),
                ("weekly:Fable", "Weekly · Fable", 16.0),
            ]
        );
        assert_eq!(
            usage.windows[1].resets_at.unwrap().as_second(),
            "2026-09-28T00:00:00Z"
                .parse::<Timestamp>()
                .unwrap()
                .as_second()
        );
    }

    #[test]
    fn codexs_read_has_the_plan_the_limit_and_the_free_resets() {
        let usage = from_codex_read(&v(CODEX_READ)).unwrap();
        assert_eq!(keys(&usage), [("weekly", "Weekly", 100.0)]);
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
        assert!(usage.limited);
        assert_eq!(usage.free_resets, Some(1));
    }

    #[test]
    fn another_models_codex_limit_is_not_the_accounts() {
        let spark = v(
            r#"{"limitId":"codex_bengalfox","primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":1788956614}}"#,
        );
        assert!(from_codex_snapshot(&spark).is_none());
        let both = v(
            r#"{"limitId":"codex","primary":{"usedPercent":40,"windowDurationMins":300},"secondary":{"usedPercent":12.5,"windowDurationMins":10080},"rateLimitReachedType":null}"#,
        );
        let usage = from_codex_snapshot(&both).unwrap();
        assert_eq!(
            keys(&usage),
            [("session", "5-hour", 40.0), ("weekly", "Weekly", 13.0)]
        );
        assert!(!usage.limited);
    }

    #[test]
    fn a_runs_event_updates_its_windows_and_keeps_the_rest() {
        let report = from_claude_report(&v(CLAUDE_REPORT)).unwrap();
        let event = from_claude_event(&v(CLAUDE_EVENT)).unwrap();
        let merged = merge(report, event);
        assert_eq!(
            keys(&merged),
            [
                ("session", "5-hour", 2.0),
                ("weekly", "Weekly", 95.0),
                ("weekly:Fable", "Weekly · Fable", 16.0),
            ]
        );
        let codex = from_codex_read(&v(CODEX_READ)).unwrap();
        let update = from_codex_snapshot(&v(
            r#"{"limitId":"codex","primary":{"usedPercent":100,"windowDurationMins":10080},"planType":null,"rateLimitReachedType":"rate_limit_reached"}"#,
        ))
        .unwrap();
        let merged = merge(codex, update);
        assert_eq!(
            merged.plan.as_deref(),
            Some("Pro"),
            "a sparse update keeps the plan"
        );
        assert_eq!(merged.free_resets, Some(1));
    }
}
