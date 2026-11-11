//! Gemini, through the Antigravity CLI (`agy`).
//!
//! Two ways in. A question, a review and a verifier run in print mode,
//! `agy --output-format stream-json`, which takes its settings for the
//! run alone with `--settings`: a terminal sandbox, the worktree read-only
//! and no network. Print mode never asks; whatever its settings do not
//! allow is refused at its edge.
//!
//! A Work run goes through `agy acp`, an Agent Client Protocol server,
//! because that is where agy asks: every file write and every command
//! comes to the client as `session/request_permission`. It takes no
//! settings and no sandbox Perch can shape, so Perch is the policy: an
//! edit inside the worktree, clear of the protected paths, is allowed
//! without asking, and everything else comes to the reader. The session
//! stays in agy's `default` mode. Its `auto` mode approves edits and
//! sandboxed commands itself, `.agents/hooks.json` among them, without a
//! request reaching Perch.
//!
//! Both load the checkout's own `.agents/` (and `.agent/`, `_agents/`):
//! hooks and MCP servers that run as the reader, outside any sandbox,
//! with no trust prompt and no flag to turn them off. So a checkout that
//! has one is refused unless the reader trusts the repository; see
//! [`repo_config`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::interactive::{ApprovalKind, ApprovalRequest, Decision, Reply};
use crate::parse::{counts_suffix, one_line, short_path, str_of};
use crate::RunEvent;

/// Where agy looks for a workspace's own customizations: hooks, MCP
/// servers, plugins, skills, rules and agents. agy 1.5.5 reads all three
/// at the top of the workspace.
pub const CONFIG_DIRS: &[&str] = &[".agents", ".agent", "_agents"];

/// The workspace configuration agy would load from `cwd`, if there is any:
/// one of [`CONFIG_DIRS`] in `cwd` or a directory above it, up to the top
/// of its repository. agy reads every one between the working directory
/// and the repository root.
pub fn repo_config(cwd: &Path) -> Option<PathBuf> {
    let real = crate::real_path(cwd);
    for dir in real.ancestors() {
        if let Some(found) = CONFIG_DIRS
            .iter()
            .map(|name| dir.join(name))
            .find(|path| path.symlink_metadata().is_ok())
        {
            return Some(found);
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    None
}

/// Why a Gemini run will not start in `cwd`, or `None` when it may.
pub(crate) fn refusal(cwd: &Path, trusted: bool) -> Option<String> {
    if trusted {
        return None;
    }
    let found = repo_config(cwd)?;
    let name = found
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(format!(
        "This checkout has its own Antigravity configuration ({name}/). agy runs its hooks and \
MCP servers as you, outside any sandbox, and has no way to turn them off, so Perch does not \
run Gemini here. Gemini runs here only as Work, with the repository trusted in config.toml \
(`[repo.work] trust = true`)."
    ))
}

/// The effort as agy spells it. agy's levels stop at `max`; Ultra, which
/// for Claude Code and Codex adds delegation, asks for its deepest level.
pub(crate) fn effort_name(effort: crate::Effort) -> &'static str {
    match effort {
        crate::Effort::Ultra => "max",
        other => other.name(),
    }
}

/// ACP's thinking level, which has only `low`, `medium` and `high`.
fn thought_level(effort: &str) -> &'static str {
    match effort {
        "low" => "low",
        "medium" => "medium",
        _ => "high",
    }
}

/// The `--settings` a print-mode run starts with. They outrank the
/// user's file for the run and are never saved. The terminal sandbox is
/// on and commands run in it without asking; the worktree and `dirs` are
/// mounted read-only by a deny rule, which outranks any allow; the
/// sandbox has no network; and web reads, browser use and MCP tools are
/// denied by name, since they run outside the sandbox. Anything else that
/// would ask is refused, print mode having nobody to ask.
pub fn read_only_settings(cwd: &Path, dirs: &[PathBuf]) -> String {
    let deny_write = std::iter::once(cwd)
        .chain(dirs.iter().map(PathBuf::as_path))
        .map(|path| format!("write_file({})", crate::real_path(path).display()));
    let deny: Vec<String> = deny_write
        .chain(
            ["read_url(*)", "execute_url(*)", "mcp(*)", "unsandboxed(*)"]
                .into_iter()
                .map(str::to_string),
        )
        .collect();
    json!({
        "agentMode": "",
        "toolPermission": "proceed-in-sandbox",
        "enableTerminalSandbox": true,
        "allowNonWorkspaceAccess": false,
        "permissions": {"allow": [], "ask": [], "deny": deny},
    })
    .to_string()
}

/// The arguments every print-mode run shares, before the model's.
///
/// A linked worktree's git directories sit outside it, where agy's
/// sandbox reads nothing, and git there says "not a git repository". They
/// go in read-only with the rest.
pub(crate) fn print_args(cwd: &Path, dirs: &[PathBuf], model: &str, effort: &str) -> Vec<String> {
    let mut dirs = dirs.to_vec();
    dirs.extend(crate::linked_git_dirs(cwd));
    let mut args: Vec<String> = vec![
        "--output-format".into(),
        "stream-json".into(),
        "--disable-slash-commands".into(),
        "--settings".into(),
        read_only_settings(cwd, &dirs),
        "--model".into(),
        model.to_string(),
        "--effort".into(),
        effort.to_string(),
    ];
    for dir in &dirs {
        args.push("--add-dir".into());
        args.push(crate::real_path(dir).to_string_lossy().into_owned());
    }
    args
}

/// A JSON schema as Gemini's API takes it: it refuses a `null` in an
/// `enum` ("cannot be empty"), so those go. A `"type": [.., "null"]`
/// beside it still allows the null.
pub(crate) fn api_schema(schema: &str) -> String {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map.iter_mut() {
                    match inner {
                        Value::Array(items) if key == "enum" => items.retain(|v| !v.is_null()),
                        _ => strip(inner),
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    match serde_json::from_str::<Value>(schema) {
        Ok(mut value) => {
            strip(&mut value);
            value.to_string()
        }
        Err(_) => schema.to_string(),
    }
}

/// What a step does, for the reader: agy's tool names and argument keys,
/// which are its own (`run_command` with `CommandLine`).
pub fn summarize(name: &str, args: &Value) -> String {
    let field = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| args.get(*k).and_then(Value::as_str))
            .map(str::to_string)
    };
    let path = || {
        field(&[
            "Path",
            "AbsolutePath",
            "TargetFile",
            "File",
            "FilePath",
            "DirectoryPath",
            "SearchPath",
            "SearchDirectory",
        ])
        .map(|p| short_path(&p))
    };
    let summary = match name {
        "run_command" => format!(
            "Ran {}",
            field(&["CommandLine", "Command"]).unwrap_or_default()
        ),
        "view_file" | "read_file" | "view_file_outline" | "view_code_item" => {
            format!("Read {}", path().unwrap_or_default())
        }
        "list_dir" => format!("Listed {}", path().unwrap_or_else(|| ".".into())),
        "find_by_name" => format!("Listed {}", field(&["Pattern"]).unwrap_or_default()),
        "grep_search" => {
            let query = field(&["Query", "Pattern"]).unwrap_or_default();
            match path() {
                Some(p) => format!("Searched for {query} in {p}"),
                None => format!("Searched for {query}"),
            }
        }
        "write_file"
        | "write_to_file"
        | "replace_file_content"
        | "multi_replace_file_content"
        | "sed_file"
        | "edit_file"
        | "notebook_edit" => {
            let what = path().unwrap_or_default();
            match field(&["Content", "CodeContent"]) {
                Some(content) if matches!(name, "write_file" | "write_to_file") => format!(
                    "Edited {what} {}",
                    counts_suffix(content.lines().count(), 0)
                ),
                _ => format!("Edited {what}"),
            }
        }
        "search_web" => format!(
            "Searched the web for {}",
            field(&["query", "Query"]).unwrap_or_default()
        ),
        "read_url_content" => format!("Read {}", field(&["Url", "url"]).unwrap_or_default()),
        other => match path() {
            Some(p) => format!("{other} {p}"),
            None => other.to_string(),
        },
    };
    one_line(&summary, 120)
}

/// The first line of a failed step's error, or of its output.
fn error_line(info: &Value) -> String {
    let message = info
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("");
    one_line(message, 100)
}

/// A command step that ran says how it exited: `Command exited with code
/// N.` in its output.
fn command_failed(output: &str) -> Option<String> {
    let code = output
        .lines()
        .find_map(|l| l.trim().strip_prefix("Command exited with code "))?
        .trim_end_matches('.')
        .trim();
    (code != "0").then(|| {
        output
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| one_line(l, 100))
            .unwrap_or_default()
    })
}

// ---------------------------------------------------------------------
// Print mode
// ---------------------------------------------------------------------

/// `agy --output-format stream-json`: an `init`, then a `step_update` for
/// each step as it goes and ends, and a `result` for each turn.
pub struct GeminiParser {
    session_id: String,
    model: String,
    /// The text of each response step as it streams, by its subagent's
    /// parent step (`None` for the run's own) and its index.
    texts: HashMap<(Option<u64>, u64), String>,
    /// Tool steps that have begun.
    started: HashSet<(Option<u64>, u64)>,
}

impl Default for GeminiParser {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiParser {
    pub fn new() -> Self {
        Self {
            session_id: String::new(),
            model: String::new(),
            texts: HashMap::new(),
            started: HashSet::new(),
        }
    }

    pub fn parse_line(&mut self, line: &str) -> Vec<RunEvent> {
        match serde_json::from_str::<Value>(line) {
            Ok(v) => self.parse_value(&v),
            Err(_) => Vec::new(),
        }
    }

    pub(crate) fn parse_value(&mut self, v: &Value) -> Vec<RunEvent> {
        match v.get("event").and_then(Value::as_str) {
            Some("init") => {
                self.session_id = str_of(v, "conversation_id");
                let init = v.get("init").unwrap_or(&Value::Null);
                self.model = str_of(init, "model");
                vec![RunEvent::Started {
                    session_id: self.session_id.clone(),
                    model: self.model.clone(),
                }]
            }
            Some("step_update") => self.step(v.get("step_update").unwrap_or(&Value::Null)),
            Some("result") => self.result(v.get("result").unwrap_or(&Value::Null)),
            _ => Vec::new(),
        }
    }

    fn step(&mut self, s: &Value) -> Vec<RunEvent> {
        let index = s.get("step_index").and_then(Value::as_u64).unwrap_or(0);
        let done = str_of(s, "state") == "DONE";
        let parent = s
            .pointer("/subagent_info/parent_step_index")
            .and_then(Value::as_u64);
        let parent_id = parent.map(|i| format!("step-{i}"));
        // A subagent counts its own steps from zero, so its are kept
        // apart from the run's, and what it says is its parent's to use,
        // not the answer.
        let key = (parent, index);
        match str_of(s, "step_type").as_str() {
            "agent_response" => {
                let text = self.texts.entry(key).or_default();
                text.push_str(&str_of(s, "text_delta"));
                if !done {
                    return Vec::new();
                }
                let text = self.texts.remove(&key).unwrap_or_default();
                if text.trim().is_empty() || parent.is_some() {
                    Vec::new()
                } else {
                    vec![RunEvent::Text(text.trim_end().to_string())]
                }
            }
            "tool" => {
                let info = s.get("tool_info").cloned().unwrap_or(Value::Null);
                let name = match str_of(s, "tool_name") {
                    n if n.is_empty() => str_of(&info, "name"),
                    n => n,
                };
                let id = match parent {
                    Some(p) => format!("step-{p}-{index}"),
                    None => format!("step-{index}"),
                };
                let call = RunEvent::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    summary: summarize(&name, info.get("parameters").unwrap_or(&Value::Null)),
                    parent_id,
                };
                let mut events = Vec::new();
                if self.started.insert(key) {
                    events.push(call);
                }
                if done {
                    let output = str_of(&info, "output");
                    let failure = if info.get("error").is_some_and(|e| !e.is_null()) {
                        Some(error_line(&info))
                    } else if name == "run_command" {
                        command_failed(&output)
                    } else {
                        None
                    };
                    events.push(RunEvent::ToolResult {
                        id,
                        ok: failure.is_none(),
                        summary: failure.unwrap_or_default(),
                    });
                }
                events
            }
            _ => Vec::new(),
        }
    }

    fn result(&mut self, r: &Value) -> Vec<RunEvent> {
        let mut events = Vec::new();
        // A structured result is the run's answer: what a review is read
        // from. It goes as the last thing said, fenced, so the review's
        // reader finds it after whatever prose came first.
        if let Some(structured) = r.get("structured_output").filter(|v| !v.is_null()) {
            events.push(RunEvent::Text(format!(
                "```json\n{}\n```",
                serde_json::to_string_pretty(structured).unwrap_or_default()
            )));
        }
        // What agy refused without a step to show for it, like a read
        // outside the workspace. Without these a turn can end in silence.
        for denied in r
            .get("denied_actions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let tool = match str_of(denied, "display_name") {
                name if name.is_empty() => str_of(denied, "action"),
                name => name,
            };
            events.push(RunEvent::PermissionDenied {
                tool: (!tool.is_empty()).then_some(tool),
                message: "agy did not allow it".into(),
            });
        }
        if let Some(usage) = r.get("usage") {
            events.push(RunEvent::Usage {
                input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
                output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
                cached_input_tokens: usage.get("cache_read_tokens").and_then(Value::as_u64),
            });
        }
        let session_id = match str_of(r, "conversation_id") {
            id if id.is_empty() => self.session_id.clone(),
            id => id,
        };
        let duration_ms = r
            .get("duration_seconds")
            .and_then(Value::as_f64)
            .map(|s| (s * 1000.0) as u64)
            .unwrap_or(0);
        match str_of(r, "status").as_str() {
            "SUCCESS" => events.push(RunEvent::Finished {
                session_id,
                cost_usd: None,
                duration_ms,
            }),
            status => {
                let error = str_of(r, "error");
                events.push(RunEvent::Failed {
                    message: if error.trim().is_empty() {
                        format!("agy ended the turn as {}", status.to_ascii_lowercase())
                    } else {
                        one_line(&error, 300)
                    },
                })
            }
        }
        events
    }
}

/// A print-mode run held on stdin, `--input-format stream-json`: the
/// prompt is the first user message, and the reader's steers wait for
/// the turn to end, then go as the next. agy reads each as a new turn
/// and says so with a `user_input` step.
pub(crate) struct GeminiHost {
    prompt: String,
    /// Steers not yet sent, oldest first.
    queued: VecDeque<String>,
    /// Steers sent and not yet read.
    sent: VecDeque<String>,
    /// `user_input` steps seen; the first is the prompt.
    inputs: usize,
}

impl GeminiHost {
    pub fn new(prompt: String) -> Self {
        Self {
            prompt,
            queued: VecDeque::new(),
            sent: VecDeque::new(),
            inputs: 0,
        }
    }

    pub fn opening(&mut self) -> Vec<String> {
        vec![user_message(&self.prompt)]
    }

    pub fn steer(&mut self, text: &str) -> Vec<String> {
        self.queued.push_back(text.to_string());
        Vec::new()
    }

    pub fn after_line(&mut self, v: &Value) -> Reply {
        match v.get("event").and_then(Value::as_str) {
            Some("step_update") => {
                let s = v.get("step_update").unwrap_or(&Value::Null);
                if str_of(s, "step_type") != "user_input"
                    || str_of(s, "state") != "DONE"
                    || s.get("subagent_info").is_some_and(|i| !i.is_null())
                {
                    return Reply::default();
                }
                self.inputs += 1;
                if self.inputs == 1 {
                    return Reply::default();
                }
                Reply {
                    events: self
                        .sent
                        .pop_front()
                        .map(RunEvent::Steered)
                        .into_iter()
                        .collect(),
                    ..Reply::default()
                }
            }
            Some("result") => {
                let failed = v.pointer("/result/status").and_then(Value::as_str) != Some("SUCCESS");
                match self.queued.pop_front() {
                    Some(next) if !failed => {
                        let write = user_message(&next);
                        self.sent.push_back(next);
                        Reply {
                            writes: vec![write],
                            more: true,
                            ..Reply::default()
                        }
                    }
                    _ => Reply {
                        close: true,
                        ..Reply::default()
                    },
                }
            }
            _ => Reply::default(),
        }
    }
}

fn user_message(text: &str) -> String {
    json!({"event": "user", "message": {"role": "user", "content": text}}).to_string()
}

// ---------------------------------------------------------------------
// ACP, for Work
// ---------------------------------------------------------------------

/// What a Work session starts with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AcpSession {
    pub cwd: PathBuf,
    pub model: String,
    /// agy's name for the effort; ACP takes it as a thinking level.
    pub effort: String,
    pub prompt: String,
    /// The session to resume, for a follow-up.
    pub resume: Option<String>,
    /// Where an edit may go without asking: the worktree and the grant's
    /// directories, symlinks resolved.
    pub writable: Vec<PathBuf>,
}

const INITIALIZE: u64 = 1;
const SESSION: u64 = 2;
const SET_MODEL: u64 = 3;
const SET_THOUGHT: u64 = 4;
const SET_MODE: u64 = 5;
/// Prompts count up from here: the first, then one per steer.
const PROMPTS: u64 = 100;

/// `agy acp` over stdio. Perch opens or resumes one session, pins its
/// model, thinking level and mode, sends the prompt, and closes stdin
/// once the last prompt is answered, which ends the server.
pub(crate) struct AcpClient {
    session: AcpSession,
    session_id: String,
    next_prompt: u64,
    /// Prompts sent and not yet answered.
    open_prompts: HashSet<u64>,
    /// The reader's messages not yet sent, and those sent but not yet
    /// echoed back as read.
    queued: VecDeque<String>,
    sent: VecDeque<String>,
    /// Each steer's prompt by its request id: an answered prompt was read
    /// whether or not agy echoed it.
    steer_ids: HashMap<u64, String>,
    /// The agent's text since its last step.
    text: String,
    /// Tool calls that have begun, and those that have ended.
    started: HashSet<String>,
    ended: HashSet<String>,
    /// Each pending request by Perch's id: the JSON-RPC id to answer, and
    /// the options it offered.
    pending: HashMap<String, (Value, Vec<Value>)>,
    usage: Option<Value>,
    /// The reader refused a request and agy has said nothing since: agy
    /// ends its turn on a refusal, silently.
    refused: bool,
    done: bool,
}

impl AcpClient {
    pub fn new(session: AcpSession) -> Self {
        Self {
            session,
            session_id: String::new(),
            next_prompt: PROMPTS,
            open_prompts: HashSet::new(),
            queued: VecDeque::new(),
            sent: VecDeque::new(),
            steer_ids: HashMap::new(),
            text: String::new(),
            started: HashSet::new(),
            ended: HashSet::new(),
            pending: HashMap::new(),
            usage: None,
            refused: false,
            done: false,
        }
    }

    pub fn opening(&mut self) -> Vec<String> {
        vec![request(
            INITIALIZE,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": {"readTextFile": false, "writeTextFile": false},
                    "terminal": false,
                },
                "clientInfo": {"name": "perch", "version": env!("CARGO_PKG_VERSION")},
            }),
        )]
    }

    /// agy steers a running turn with another prompt. One sent before
    /// the session exists waits for it.
    pub fn steer(&mut self, text: &str) -> Vec<String> {
        if self.done {
            return Vec::new();
        }
        self.queued.push_back(text.to_string());
        if self.session_id.is_empty() || self.open_prompts.is_empty() {
            return Vec::new();
        }
        self.send_queued()
    }

    fn send_queued(&mut self) -> Vec<String> {
        let texts: Vec<String> = self.queued.drain(..).collect();
        texts
            .into_iter()
            .map(|text| {
                let write = self.prompt(&text);
                self.steer_ids.insert(self.next_prompt - 1, text.clone());
                self.sent.push_back(text);
                write
            })
            .collect()
    }

    fn prompt(&mut self, text: &str) -> String {
        let id = self.next_prompt;
        self.next_prompt += 1;
        self.open_prompts.insert(id);
        request(
            id,
            "session/prompt",
            json!({
                "sessionId": self.session_id,
                "prompt": [{"type": "text", "text": text}],
            }),
        )
    }

    fn set_option(&self, id: u64, config: &str, value: &str) -> String {
        request(
            id,
            "session/set_config_option",
            json!({"sessionId": self.session_id, "configId": config, "value": value}),
        )
    }

    /// What the agent said since its last step, as one message.
    fn flush_text(&mut self) -> Vec<RunEvent> {
        let text = std::mem::take(&mut self.text);
        if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![RunEvent::Text(text.trim_end().to_string())]
        }
    }

    fn end(&mut self, event: RunEvent) -> Reply {
        self.done = true;
        let mut events = self.flush_text();
        if let Some(usage) = self.usage.take() {
            events.push(RunEvent::Usage {
                input_tokens: usage.get("inputTokens").and_then(Value::as_u64),
                output_tokens: usage.get("outputTokens").and_then(Value::as_u64),
                cached_input_tokens: usage.get("cachedReadTokens").and_then(Value::as_u64),
            });
        }
        events.push(event);
        Reply {
            events,
            close: true,
            ..Reply::default()
        }
    }

    fn fail(&mut self, message: String) -> Reply {
        self.end(RunEvent::Failed { message })
    }

    pub fn after_line(&mut self, v: &Value) -> Reply {
        if self.done {
            return Reply::default();
        }
        let method = v.get("method").and_then(Value::as_str);
        match (method, v.get("id")) {
            (None, Some(id)) => self.response(id.as_u64().unwrap_or(0), v),
            (Some(method), Some(id)) => self.server_request(id.clone(), method, v),
            (Some("session/update"), None) => {
                self.update(v.pointer("/params/update").unwrap_or(&Value::Null))
            }
            _ => Reply::default(),
        }
    }

    fn response(&mut self, id: u64, v: &Value) -> Reply {
        // A steer agy would not take is left unread, for Perch to send as
        // a follow-up, rather than failing the run it came into.
        if v.get("error").is_some() && self.steer_ids.contains_key(&id) {
            let text = self.steer_ids.remove(&id).unwrap_or_default();
            if let Some(at) = self.sent.iter().position(|s| *s == text) {
                self.sent.remove(at);
            }
            if !self.open_prompts.remove(&id) || !self.open_prompts.is_empty() {
                return Reply::default();
            }
            return self.turn_over(&Value::Null, Vec::new());
        }
        if let Some(error) = v.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("agy refused the request")
                .to_string();
            return self.fail(one_line(&message, 300));
        }
        let result = v.get("result").cloned().unwrap_or(Value::Null);
        match id {
            INITIALIZE => {
                let params = json!({
                    "cwd": self.session.cwd.to_string_lossy(),
                    "mcpServers": [],
                });
                let next = match &self.session.resume {
                    Some(id) => {
                        let mut params = params;
                        params["sessionId"] = json!(id);
                        request(SESSION, "session/resume", params)
                    }
                    None => request(SESSION, "session/new", params),
                };
                Reply {
                    writes: vec![next],
                    ..Reply::default()
                }
            }
            SESSION => {
                self.session_id = result
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| self.session.resume.clone())
                    .unwrap_or_default();
                Reply {
                    writes: vec![self.set_option(SET_MODEL, "model", &self.session.model)],
                    events: vec![RunEvent::Started {
                        session_id: self.session_id.clone(),
                        model: self.session.model.clone(),
                    }],
                    ..Reply::default()
                }
            }
            SET_MODEL => Reply {
                writes: vec![self.set_option(
                    SET_THOUGHT,
                    "thought_level",
                    thought_level(&self.session.effort),
                )],
                ..Reply::default()
            },
            SET_THOUGHT => Reply {
                writes: vec![request(
                    SET_MODE,
                    "session/set_mode",
                    json!({"sessionId": self.session_id, "modeId": "default"}),
                )],
                ..Reply::default()
            },
            SET_MODE => {
                let prompt = self.session.prompt.clone();
                let mut writes = vec![self.prompt(&prompt)];
                writes.extend(self.send_queued());
                Reply {
                    writes,
                    ..Reply::default()
                }
            }
            id if self.open_prompts.remove(&id) => {
                let mut steered = Vec::new();
                if let Some(text) = self.steer_ids.remove(&id) {
                    if let Some(at) = self.sent.iter().position(|s| *s == text) {
                        self.sent.remove(at);
                        steered = self.flush_text();
                        steered.push(RunEvent::Steered(text));
                    }
                }
                if let Some(usage) = result.get("usage").filter(|u| u.is_object()) {
                    self.usage = Some(usage.clone());
                }
                if !self.open_prompts.is_empty() {
                    return Reply {
                        events: steered,
                        ..Reply::default()
                    };
                }
                self.turn_over(&result, steered)
            }
            _ => Reply::default(),
        }
    }

    /// The last open prompt is answered: send what came in meanwhile, or
    /// end the run.
    fn turn_over(&mut self, result: &Value, steered: Vec<RunEvent>) -> Reply {
        let mut reply = self.turn_end(result);
        let mut events = steered;
        events.append(&mut reply.events);
        reply.events = events;
        reply
    }

    fn turn_end(&mut self, result: &Value) -> Reply {
        // A steer that came as the turn ended goes as the next.
        if !self.queued.is_empty() {
            let mut reply = Reply {
                writes: self.send_queued(),
                ..Reply::default()
            };
            reply.events = self.flush_text();
            return reply;
        }
        if result.get("stopReason").and_then(Value::as_str) == Some("refusal") {
            return self.fail("The model refused to go on".into());
        }
        let stopped = self.refused && self.text.trim().is_empty();
        let session_id = self.session_id.clone();
        let mut reply = self.end(RunEvent::Finished {
            session_id,
            cost_usd: None,
            duration_ms: 0,
        });
        if stopped {
            let at = reply.events.len().saturating_sub(1);
            reply.events.insert(
                at,
                RunEvent::PermissionDenied {
                    tool: None,
                    message: "Gemini ends its turn when a request is refused. Reply to carry on."
                        .into(),
                },
            );
        }
        reply
    }

    fn update(&mut self, u: &Value) -> Reply {
        let events = match str_of(u, "sessionUpdate").as_str() {
            "agent_message_chunk" => {
                if let Some(text) = u.pointer("/content/text").and_then(Value::as_str) {
                    self.text.push_str(text);
                    self.refused &= text.trim().is_empty();
                }
                Vec::new()
            }
            "user_message_chunk" => {
                let text = u.pointer("/content/text").and_then(Value::as_str);
                match (text, self.sent.front()) {
                    (Some(text), Some(front)) if text == front => {
                        let mut events = self.flush_text();
                        events.extend(self.sent.pop_front().map(RunEvent::Steered));
                        events
                    }
                    _ => Vec::new(),
                }
            }
            "tool_call" | "tool_call_update" => self.tool(u),
            _ => Vec::new(),
        };
        Reply {
            events,
            ..Reply::default()
        }
    }

    fn tool(&mut self, u: &Value) -> Vec<RunEvent> {
        let id = str_of(u, "toolCallId");
        if id.is_empty() || self.ended.contains(&id) {
            return Vec::new();
        }
        let mut events = self.flush_text();
        if self.started.insert(id.clone()) {
            self.refused = false;
            events.push(RunEvent::ToolCall {
                id: id.clone(),
                name: tool_name(u),
                summary: tool_summary(u),
                parent_id: None,
            });
        }
        let status = str_of(u, "status");
        if matches!(status.as_str(), "completed" | "failed") {
            self.ended.insert(id.clone());
            let output = u
                .pointer("/rawOutput/output")
                .and_then(Value::as_str)
                .unwrap_or("");
            let exit = u.pointer("/rawOutput/exit_code").and_then(Value::as_i64);
            let failed = status == "failed" || exit.is_some_and(|code| code != 0);
            let failure = failed.then(|| crate::codex::last_line(output));
            events.push(RunEvent::ToolResult {
                id,
                ok: failure.is_none(),
                summary: failure.unwrap_or_default(),
            });
        }
        events
    }

    fn server_request(&mut self, id: Value, method: &str, v: &Value) -> Reply {
        if method != "session/request_permission" {
            // Perch offers no file system or terminal of its own.
            return Reply {
                writes: vec![json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": "Perch does not handle this request"},
                })
                .to_string()],
                ..Reply::default()
            };
        }
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        let call = params.get("toolCall").cloned().unwrap_or(Value::Null);
        let options: Vec<Value> = params
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut events = self.flush_text();
        if let Some(paths) = edit_paths(&call) {
            if !paths.is_empty()
                && paths
                    .iter()
                    .all(|path| may_edit(path, &self.session.writable))
            {
                return Reply {
                    writes: answer_line(&id, &options, true).into_iter().collect(),
                    events,
                    ..Reply::default()
                };
            }
        }
        let key = match &id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        self.pending.insert(key.clone(), (id, options));
        events.push(RunEvent::Approval {
            id: key,
            request: approval(&call, &self.session.writable),
        });
        Reply {
            events,
            ..Reply::default()
        }
    }

    pub fn answer(&mut self, id: &str, decision: &Decision) -> Option<String> {
        let (rpc_id, options) = self.pending.remove(id)?;
        let allow = matches!(decision, Decision::Allow);
        self.refused |= !allow;
        answer_line(&rpc_id, &options, allow)
    }
}

/// The reply that picks a request's one-time allow or reject, by the id
/// it offered for it, or cancels it when it offered none.
fn answer_line(id: &Value, options: &[Value], allow: bool) -> Option<String> {
    let kind = if allow { "allow_once" } else { "reject_once" };
    let outcome = options
        .iter()
        .find(|o| str_of(o, "kind") == kind)
        .map(|o| json!({"outcome": "selected", "optionId": str_of(o, "optionId")}))
        .unwrap_or_else(|| json!({"outcome": "cancelled"}));
    Some(json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": outcome}}).to_string())
}

fn tool_name(u: &Value) -> String {
    match str_of(u, "name") {
        n if n.is_empty() => str_of(u, "kind"),
        n => n,
    }
}

fn tool_args(u: &Value) -> Value {
    u.get("rawInput")
        .filter(|v| v.is_object())
        .or_else(|| u.get("args"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn tool_summary(u: &Value) -> String {
    let name = tool_name(u);
    let summary = summarize(&name, &tool_args(u));
    if summary == name {
        match str_of(u, "title") {
            t if t.trim().is_empty() => summary,
            t => one_line(&t, 120),
        }
    } else {
        summary
    }
}

/// The files an edit request writes, when it is one: ACP's `edit` kind,
/// with its locations, or the path in its input.
fn edit_paths(call: &Value) -> Option<Vec<PathBuf>> {
    if str_of(call, "kind") != "edit" {
        return None;
    }
    let mut paths: Vec<PathBuf> = call
        .get("locations")
        .and_then(Value::as_array)
        .map(|locations| {
            locations
                .iter()
                .map(|l| str_of(l, "path"))
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    let args = tool_args(call);
    for key in [
        "Path",
        "TargetFile",
        "AbsolutePath",
        "File",
        "FilePath",
        "path",
        "file_path",
        "target_file",
    ] {
        if let Some(path) = args.get(key).and_then(Value::as_str) {
            let path = PathBuf::from(path);
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    Some(paths)
}

/// `path` with `..` and `.` resolved and the longest part that exists
/// taken through its symlinks: a file the edit creates does not exist
/// yet. `None` for a relative path, or one that climbs above the root.
fn resolve(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut lexical = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !lexical.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            other => lexical.push(other),
        }
    }
    let mut existing = lexical.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(existing) {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        rest.push(existing.file_name()?.to_os_string());
        existing = existing.parent()?;
    }
}

/// Whether an edit to `path` may go without asking: under one of
/// `writable`, with no protected name anywhere below that root. Names
/// compare without case, as macOS's filesystem does, so `.Claude` is
/// `.claude`; and at any depth, so a grant of the worktree's parent
/// cannot reach the worktree's own `.claude`.
fn may_edit(path: &Path, writable: &[PathBuf]) -> bool {
    let Some(path) = resolve(path) else {
        return false;
    };
    writable.iter().any(|root| {
        let Ok(inside) = path.strip_prefix(root) else {
            return false;
        };
        inside.components().next().is_some()
            && !inside.components().any(|part| {
                let part = part.as_os_str().to_string_lossy();
                crate::WORK_PROTECTED
                    .iter()
                    .any(|name| part.eq_ignore_ascii_case(name))
            })
    })
}

/// A permission request as the card shows it.
fn approval(call: &Value, writable: &[PathBuf]) -> ApprovalRequest {
    let name = tool_name(call);
    let args = tool_args(call);
    let title = str_of(call, "title");
    match str_of(call, "kind").as_str() {
        "execute" => ApprovalRequest {
            kind: ApprovalKind::Command,
            subject: args
                .get("CommandLine")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(title),
            detail: args
                .get("toolSummary")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(|s| one_line(s, 200)),
            suggestion: None,
        },
        "edit" => {
            // Every path that needs asking, so none hides behind one that
            // would have gone anyway, and "Always" keeps exactly that set.
            let asked: Vec<PathBuf> = edit_paths(call)
                .unwrap_or_default()
                .into_iter()
                .filter(|p| !may_edit(p, writable))
                .collect();
            let subject = if asked.is_empty() {
                // No path given: the request as sent, so a grant for it
                // covers nothing broader.
                format!("{name} {}", one_line(&args.to_string(), 200))
            } else {
                asked
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let inside = !asked.is_empty()
                && asked.iter().all(|p| {
                    resolve(p).is_some_and(|p| writable.iter().any(|root| p.starts_with(root)))
                });
            ApprovalRequest {
                kind: ApprovalKind::Edit,
                subject,
                detail: inside.then(|| "A protected path in the worktree".to_string()),
                suggestion: None,
            }
        }
        _ => ApprovalRequest {
            subject: match summarize(&name, &args) {
                s if s == name && !title.trim().is_empty() => one_line(&title, 120),
                s => s,
            },
            kind: ApprovalKind::Tool { name },
            detail: None,
            suggestion: None,
        },
    }
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn print_mode_steps_become_events() {
        let mut p = GeminiParser::new();
        let started = p.parse_line(
            r#"{"event":"init","conversation_id":"c-1","init":{"model":"gemini-3.8-flash","cwd":"/wt","tools":[],"permission_mode":"proceed-in-sandbox"}}"#,
        );
        assert_eq!(
            started,
            vec![RunEvent::Started {
                session_id: "c-1".into(),
                model: "gemini-3.8-flash".into()
            }]
        );
        let call = p.parse_line(
            r#"{"event":"step_update","step_update":{"conversation_id":"c-1","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"git log -1"}}}}"#,
        );
        assert!(
            matches!(&call[..], [RunEvent::ToolCall { id, summary, .. }] if id == "step-2" && summary == "Ran git log -1"),
            "{call:?}"
        );
        let failed = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"git log -1"},"output":"Created At: x\nCommand exited with code 128.\n\nCommand output:\nfatal: bad revision\r\n"}}}"#,
        );
        assert_eq!(
            failed,
            vec![RunEvent::ToolResult {
                id: "step-2".into(),
                ok: false,
                summary: "fatal: bad revision".into()
            }]
        );
        let refused = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"tool","tool_name":"write_file","tool_info":{"name":"write_file","parameters":{"Path":"/wt/d.txt","Content":"y"},"error":{"type":"permission","message":"Permission denied for write_file(/wt/d.txt)"}}}}"#,
        );
        assert!(
            matches!(
                &refused[..],
                [RunEvent::ToolCall { summary, .. }, RunEvent::ToolResult { ok: false, summary: why, .. }]
                    if summary == "Edited wt/d.txt (+1 \u{2212}0)" && why.starts_with("Permission denied")
            ),
            "{refused:?}"
        );
        for delta in ["The answer", " is 4.", "\n"] {
            let partial = p.parse_line(&format!(
                r#"{{"event":"step_update","step_update":{{"step_index":4,"state":"ACTIVE","step_type":"agent_response","text_delta":{}}}}}"#,
                serde_json::to_string(delta).unwrap()
            ));
            assert!(partial.is_empty());
        }
        let said = p.parse_line(
            r#"{"event":"step_update","step_update":{"step_index":4,"state":"DONE","step_type":"agent_response","text_delta":""}}"#,
        );
        assert_eq!(said, vec![RunEvent::Text("The answer is 4.".into())]);
        let done = p.parse_line(
            r#"{"event":"result","result":{"conversation_id":"c-1","status":"SUCCESS","response":"The answer is 4.","duration_seconds":2.5,"num_turns":1,"usage":{"input_tokens":10,"output_tokens":3,"cache_read_tokens":1}}}"#,
        );
        assert_eq!(
            done,
            vec![
                RunEvent::Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(3),
                    cached_input_tokens: Some(1)
                },
                RunEvent::Finished {
                    session_id: "c-1".into(),
                    cost_usd: None,
                    duration_ms: 2500
                }
            ]
        );
    }

    #[test]
    fn a_structured_result_is_said_and_an_error_fails_the_run() {
        let mut p = GeminiParser::new();
        let review = p.parse_line(
            r#"{"event":"result","result":{"conversation_id":"c","status":"SUCCESS","structured_output":{"status":"clean","findings":[]}}}"#,
        );
        assert!(
            matches!(&review[0], RunEvent::Text(t) if t.contains("\"clean\"")),
            "{review:?}"
        );
        let error = p.parse_line(
            r#"{"event":"result","result":{"conversation_id":"","status":"ERROR","error":"model x is not recognized"}}"#,
        );
        assert_eq!(
            error,
            vec![RunEvent::Failed {
                message: "model x is not recognized".into()
            }]
        );
    }

    #[test]
    fn the_print_host_holds_steers_for_the_next_turn() {
        let mut host = GeminiHost::new("Why?".into());
        let opening: Value = serde_json::from_str(&host.opening()[0]).unwrap();
        assert_eq!(opening["event"], "user");
        assert_eq!(opening["message"]["content"], "Why?");
        let prompt = host.after_line(&line(
            r#"{"event":"step_update","step_update":{"step_index":0,"state":"DONE","step_type":"user_input"}}"#,
        ));
        assert_eq!(prompt, Reply::default());
        assert!(
            host.steer("And how?").is_empty(),
            "held until the turn ends"
        );
        let turn = host.after_line(&line(r#"{"event":"result","result":{"status":"SUCCESS"}}"#));
        assert!(turn.more && !turn.close);
        let next: Value = serde_json::from_str(&turn.writes[0]).unwrap();
        assert_eq!(next["message"]["content"], "And how?");
        let read = host.after_line(&line(
            r#"{"event":"step_update","step_update":{"step_index":5,"state":"DONE","step_type":"user_input"}}"#,
        ));
        assert_eq!(read.events, vec![RunEvent::Steered("And how?".into())]);
        let end = host.after_line(&line(r#"{"event":"result","result":{"status":"SUCCESS"}}"#));
        assert!(end.close && !end.more);
    }

    #[test]
    fn read_only_settings_deny_writes_and_the_web() {
        let dir = tempfile::tempdir().unwrap();
        let settings: Value = serde_json::from_str(&read_only_settings(dir.path(), &[])).unwrap();
        assert_eq!(settings["enableTerminalSandbox"], true);
        assert_eq!(settings["toolPermission"], "proceed-in-sandbox");
        let deny: Vec<&str> = settings["permissions"]["deny"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let real = crate::real_path(dir.path());
        assert!(deny.contains(&format!("write_file({})", real.display()).as_str()));
        for rule in ["read_url(*)", "execute_url(*)", "mcp(*)"] {
            assert!(deny.contains(&rule), "{rule}");
        }
        assert!(settings["permissions"]["allow"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_checkout_with_agy_config_is_refused_unless_trusted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(refusal(dir.path(), false), None);
        for name in CONFIG_DIRS {
            let config = dir.path().join(name);
            std::fs::create_dir(&config).unwrap();
            let why = refusal(dir.path(), false).expect("refused");
            assert!(why.contains(&format!("({name}/)")), "{why}");
            assert_eq!(refusal(dir.path(), true), None);
            std::fs::remove_dir(&config).unwrap();
        }
        // One in a parent inside the repository counts; one above it not.
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::create_dir(dir.path().join(".agents")).unwrap();
        assert!(refusal(&sub, false).is_some());
    }

    fn session(dir: &Path) -> AcpSession {
        AcpSession {
            cwd: dir.to_path_buf(),
            model: "gemini-4-carbon".into(),
            effort: "max".into(),
            prompt: "Fix it".into(),
            resume: None,
            writable: vec![crate::real_path(dir)],
        }
    }

    fn written(reply: &Reply) -> Value {
        serde_json::from_str(&reply.writes[0]).unwrap()
    }

    #[test]
    fn acp_opens_a_session_pins_it_and_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        let init: Value = serde_json::from_str(&acp.opening()[0]).unwrap();
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["params"]["clientCapabilities"]["terminal"], false);
        let new = written(&acp.after_line(&line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)));
        assert_eq!(new["method"], "session/new");
        assert_eq!(new["params"]["mcpServers"], json!([]));
        let opened = acp.after_line(&line(r#"{"id":2,"result":{"sessionId":"s-1"}}"#));
        assert_eq!(
            opened.events,
            vec![RunEvent::Started {
                session_id: "s-1".into(),
                model: "gemini-4-carbon".into()
            }]
        );
        let model = written(&opened);
        assert_eq!(model["method"], "session/set_config_option");
        assert_eq!(model["params"]["configId"], "model");
        assert_eq!(model["params"]["value"], "gemini-4-carbon");
        let thought = written(&acp.after_line(&line(r#"{"id":3,"result":{}}"#)));
        assert_eq!(thought["params"]["configId"], "thought_level");
        assert_eq!(thought["params"]["value"], "high");
        let mode = written(&acp.after_line(&line(r#"{"id":4,"result":{}}"#)));
        assert_eq!(mode["method"], "session/set_mode");
        assert_eq!(mode["params"]["modeId"], "default");
        let prompt = written(&acp.after_line(&line(r#"{"id":5,"result":{}}"#)));
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["prompt"][0]["text"], "Fix it");

        let chunk = acp.after_line(&line(
            r#"{"method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Looking."}}}}"#,
        ));
        assert!(chunk.events.is_empty());
        let call = acp.after_line(&line(
            r#"{"method":"session/update","params":{"update":{"sessionUpdate":"tool_call","toolCallId":"t1","name":"run_command","kind":"execute","status":"in_progress","title":"cargo test","rawInput":{"CommandLine":"cargo test"}}}}"#,
        ));
        assert_eq!(call.events[0], RunEvent::Text("Looking.".into()));
        assert!(
            matches!(&call.events[1], RunEvent::ToolCall { id, summary, .. } if id == "t1" && summary == "Ran cargo test")
        );
        let ran = acp.after_line(&line(
            r#"{"method":"session/update","params":{"update":{"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","rawOutput":{"output":"error: 1 failed\n","exit_code":101}}}}"#,
        ));
        assert_eq!(
            ran.events,
            vec![RunEvent::ToolResult {
                id: "t1".into(),
                ok: false,
                summary: "error: 1 failed".into()
            }]
        );
        let end = acp.after_line(&line(
            r#"{"id":100,"result":{"stopReason":"end_turn","usage":{"inputTokens":5,"outputTokens":2,"cachedReadTokens":1}}}"#,
        ));
        assert!(end.close);
        assert!(
            matches!(
                &end.events[..],
                [RunEvent::Usage { input_tokens: Some(5), .. }, RunEvent::Finished { session_id, .. }] if session_id == "s-1"
            ),
            "{:?}",
            end.events
        );
    }

    #[test]
    fn acp_resumes_a_session_without_replaying_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(AcpSession {
            resume: Some("s-0".into()),
            ..session(dir.path())
        });
        acp.opening();
        let resume = written(&acp.after_line(&line(r#"{"id":1,"result":{}}"#)));
        assert_eq!(resume["method"], "session/resume");
        assert_eq!(resume["params"]["sessionId"], "s-0");
        let opened = acp.after_line(&line(r#"{"id":2,"result":{}}"#));
        assert!(
            matches!(&opened.events[0], RunEvent::Started { session_id, .. } if session_id == "s-0")
        );
    }

    #[test]
    fn acp_edits_in_the_worktree_go_and_the_rest_come_to_the_reader() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::real_path(dir.path());
        let mut acp = AcpClient::new(session(dir.path()));
        let ask = |path: &str, id: u64| {
            format!(
                r#"{{"jsonrpc":"2.0","method":"session/request_permission","id":"{id}","params":{{"sessionId":"s","toolCall":{{"toolCallId":"w{id}","name":"write_file","kind":"edit","title":"File","locations":[{{"path":"{path}"}}],"rawInput":{{"Path":"{path}","Content":"x"}}}},"options":[{{"optionId":"allow","name":"Allow","kind":"allow_once"}},{{"optionId":"deny","name":"Deny","kind":"reject_once"}}]}}}}"#
            )
        };
        let inside = acp.after_line(&line(&ask(&root.join("src/new.rs").to_string_lossy(), 1)));
        assert!(inside.events.is_empty(), "{:?}", inside.events);
        let allowed = written(&inside);
        assert_eq!(allowed["id"], "1");
        assert_eq!(allowed["result"]["outcome"]["optionId"], "allow");

        for (n, path) in [
            root.join(".agents/hooks.json"),
            root.join("_agents/x"),
            root.join("sub/.agents/hooks.json"),
            root.join(".git/hooks/pre-commit"),
            root.join(".claude/settings.json"),
            root.join("src/../../escape.txt"),
            PathBuf::from("/etc/hosts"),
        ]
        .iter()
        .enumerate()
        {
            let id = n as u64 + 10;
            let asked = acp.after_line(&line(&ask(&path.to_string_lossy(), id)));
            assert!(asked.writes.is_empty(), "{path:?} went without asking");
            assert!(
                matches!(
                    &asked.events[..],
                    [RunEvent::Approval {
                        request: ApprovalRequest {
                            kind: ApprovalKind::Edit,
                            ..
                        },
                        ..
                    }]
                ),
                "{path:?}: {:?}",
                asked.events
            );
        }
        let command = acp.after_line(&line(
            r#"{"jsonrpc":"2.0","method":"session/request_permission","id":"c1","params":{"toolCall":{"toolCallId":"t","name":"run_command","kind":"execute","title":"curl x","rawInput":{"CommandLine":"curl https://example.com","toolSummary":"Fetch the page"}},"options":[{"optionId":"allow","kind":"allow_once"},{"optionId":"deny","kind":"reject_once"}]}}"#,
        ));
        assert_eq!(
            command.events,
            vec![RunEvent::Approval {
                id: "c1".into(),
                request: ApprovalRequest {
                    kind: ApprovalKind::Command,
                    subject: "curl https://example.com".into(),
                    detail: Some("Fetch the page".into()),
                    suggestion: None,
                }
            }]
        );
        let denied: Value =
            serde_json::from_str(&acp.answer("c1", &Decision::deny("no")).unwrap()).unwrap();
        assert_eq!(denied["id"], "c1");
        assert_eq!(denied["result"]["outcome"]["optionId"], "deny");
        assert_eq!(acp.answer("c1", &Decision::Allow), None, "answered once");

        let other = acp.after_line(&line(
            r#"{"jsonrpc":"2.0","method":"fs/read_text_file","id":9,"params":{}}"#,
        ));
        assert!(other.writes[0].contains("\"error\""));
    }

    #[test]
    fn a_linked_worktree_s_git_dirs_are_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "one",
            ],
        );
        let wt = root.join("wt");
        git(&repo, &["worktree", "add", "-q", wt.to_str().unwrap()]);

        let args = print_args(&wt, &[], "m", "low");
        let added: Vec<&String> = args
            .windows(2)
            .filter(|w| w[0] == "--add-dir")
            .map(|w| &w[1])
            .collect();
        let git_dir = repo.join(".git").to_string_lossy().into_owned();
        assert!(added.contains(&&git_dir), "{added:?}");
        let settings = &args[args.iter().position(|a| a == "--settings").unwrap() + 1];
        assert!(
            settings.contains(&format!("write_file({git_dir})")),
            "{settings}"
        );
    }

    #[test]
    fn the_api_schema_has_no_null_in_an_enum() {
        let schema = api_schema(&crate::review_output_schema());
        assert!(
            !schema.contains(",null]") && !schema.contains("[null"),
            "{schema}"
        );
        let value: Value = serde_json::from_str(&schema).unwrap();
        assert_eq!(
            value.pointer("/properties/findings/items/properties/side/type"),
            Some(&json!(["string", "null"]))
        );
    }

    #[test]
    fn protected_names_ask_in_any_case_at_any_depth_under_any_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().canonicalize().unwrap();
        let wt = parent.join("wt");
        std::fs::create_dir(&wt).unwrap();
        let writable = vec![wt.clone(), parent.clone()];
        assert!(may_edit(&wt.join("src/lib.rs"), &writable));
        assert!(may_edit(&parent.join("notes.txt"), &writable));
        for path in [
            wt.join(".Claude/settings.json"),
            wt.join(".AGENTS/hooks.json"),
            wt.join(".MCP.json"),
            wt.join("sub/.GIT/config"),
            wt.join("packages/app/.claude/settings.json"),
            wt.join(".claude/settings.json"),
        ] {
            assert!(!may_edit(&path, &writable), "{path:?}");
        }
    }

    #[test]
    fn an_edit_card_names_every_path_that_asks() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().canonicalize().unwrap();
        let writable = vec![wt.clone()];
        let call = json!({
            "kind": "edit",
            "name": "write_file",
            "locations": [
                {"path": wt.join("README.md")},
                {"path": "/etc/hosts"},
                {"path": wt.join(".claude/settings.json")},
            ],
        });
        let request = approval(&call, &writable);
        assert_eq!(
            request.subject,
            format!("/etc/hosts, {}", wt.join(".claude/settings.json").display())
        );
        assert_eq!(request.detail, None, "not all of it is in the worktree");
        let bare = approval(
            &json!({"kind": "edit", "name": "write_file", "rawInput": {"x": 1}}),
            &writable,
        );
        assert_ne!(bare.subject, "files");
    }

    #[test]
    fn a_refusal_that_ends_the_turn_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        acp.opening();
        for id in 1..=5 {
            acp.after_line(&line(&format!(
                r#"{{"id":{id},"result":{{"sessionId":"s"}}}}"#
            )));
        }
        acp.after_line(&line(
            r#"{"jsonrpc":"2.0","method":"session/request_permission","id":"c1","params":{"toolCall":{"toolCallId":"t","name":"run_command","kind":"execute","rawInput":{"CommandLine":"rm x"}},"options":[{"optionId":"allow","kind":"allow_once"},{"optionId":"deny","kind":"reject_once"}]}}"#,
        ));
        acp.answer("c1", &Decision::deny("no"));
        let end = acp.after_line(&line(r#"{"id":100,"result":{"stopReason":"end_turn"}}"#));
        assert!(
            matches!(
                &end.events[..],
                [
                    RunEvent::PermissionDenied { tool: None, .. },
                    RunEvent::Finished { .. }
                ]
            ),
            "{:?}",
            end.events
        );
    }

    #[test]
    fn what_print_mode_refused_is_shown() {
        let mut parser = GeminiParser::new();
        let events = parser.parse_line(
            r#"{"event":"result","result":{"conversation_id":"c","status":"SUCCESS","response":"","denied_actions":[{"action":"read_file","display_name":"ReadFile"}]}}"#,
        );
        assert_eq!(
            events[0],
            RunEvent::PermissionDenied {
                tool: Some("ReadFile".into()),
                message: "agy did not allow it".into(),
            }
        );
        assert!(matches!(events.last(), Some(RunEvent::Finished { .. })));
    }

    #[test]
    fn acp_steers_go_as_prompts_and_land_when_echoed() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        acp.opening();
        for id in 1..=4 {
            acp.after_line(&line(&format!(
                r#"{{"id":{id},"result":{{"sessionId":"s"}}}}"#
            )));
        }
        assert!(acp.steer("Early").is_empty(), "no prompt going yet");
        let started = acp.after_line(&line(r#"{"id":5,"result":{}}"#));
        assert_eq!(started.writes.len(), 2, "the prompt, then the early steer");
        let steer = acp.steer("And the docs");
        let steer: Value = serde_json::from_str(&steer[0]).unwrap();
        assert_eq!(steer["method"], "session/prompt");
        let echoed = acp.after_line(&line(
            r#"{"method":"session/update","params":{"update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"Early"}}}}"#,
        ));
        assert_eq!(echoed.events, vec![RunEvent::Steered("Early".into())]);
        let first = acp.after_line(&line(r#"{"id":100,"result":{"stopReason":"end_turn"}}"#));
        assert!(!first.close, "two prompts still open");
        let echoed_early =
            acp.after_line(&line(r#"{"id":101,"result":{"stopReason":"end_turn"}}"#));
        assert!(echoed_early.events.is_empty(), "Early landed once");
        // Answered without an echo: read all the same.
        let last = acp.after_line(&line(r#"{"id":102,"result":{"stopReason":"end_turn"}}"#));
        assert!(last.close);
        assert_eq!(last.events[0], RunEvent::Steered("And the docs".into()));
    }

    #[test]
    fn a_steer_agy_will_not_take_is_left_unread() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        acp.opening();
        for id in 1..=5 {
            acp.after_line(&line(&format!(
                r#"{{"id":{id},"result":{{"sessionId":"s"}}}}"#
            )));
        }
        acp.steer("Late");
        let refused = acp.after_line(&line(
            r#"{"id":101,"error":{"code":-32603,"message":"busy"}}"#,
        ));
        assert!(!refused.close && refused.events.is_empty());
        let end = acp.after_line(&line(r#"{"id":100,"result":{"stopReason":"end_turn"}}"#));
        assert!(end.close);
        assert!(!end
            .events
            .iter()
            .any(|e| matches!(e, RunEvent::Steered(_) | RunEvent::Failed { .. })));
    }

    #[test]
    fn acp_errors_end_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut acp = AcpClient::new(session(dir.path()));
        acp.opening();
        let failed = acp.after_line(&line(
            r#"{"id":1,"error":{"code":-32603,"message":"Failed to start Antigravity"}}"#,
        ));
        assert!(failed.close);
        assert_eq!(
            failed.events,
            vec![RunEvent::Failed {
                message: "Failed to start Antigravity".into()
            }]
        );
        assert_eq!(
            acp.after_line(&line(r#"{"id":2,"result":{}}"#)),
            Reply::default()
        );
    }

    #[test]
    fn summaries_read_agy_s_own_argument_names() {
        assert_eq!(
            summarize("view_file", &json!({"AbsolutePath": "/a/b/c/d.rs"})),
            "Read b/c/d.rs"
        );
        assert_eq!(
            summarize(
                "grep_search",
                &json!({"Query": "fn main", "SearchPath": "/wt/src"})
            ),
            "Searched for fn main in wt/src"
        );
        assert_eq!(summarize("finish", &json!({})), "finish");
    }
}
