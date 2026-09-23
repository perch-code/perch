//! Conversations with a CLI over its stdin, for runs that can ask.
//!
//! A Work run started in print mode can only be refused at its sandbox's
//! edge. Started this way it asks, and Perch answers. Claude Code is
//! driven by its host permission protocol: stream-json in and out, with
//! `can_use_tool` control requests for anything its permissions would
//! otherwise prompt for. Codex is driven through `codex app-server`, a
//! JSON-RPC peer that starts a thread and a turn on request and sends
//! its approval requests to the client.
//!
//! A conversation is told each line the CLI prints. It says what to write
//! back, which events the line makes, and when stdin should close, which
//! is how both CLIs are told there is nothing more to do.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::codex::{human_message, last_line, strip_shell};
use crate::parse::{one_line, str_of, summarize_tool_call};
use crate::RunEvent;

/// What a request is about, which decides what the card calls it and
/// what an "Always" grant for it keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalKind {
    /// A connection from a sandboxed command to a host.
    Host,
    /// A command to run outside the sandbox.
    Command,
    /// A change to a file outside the worktree.
    Edit,
    /// Any other tool, by its name.
    Tool { name: String },
}

/// One thing the agent wants to do that its sandbox would not let it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub kind: ApprovalKind,
    /// The host, the command, the path, or what the tool is about.
    pub subject: String,
    /// The agent's reason, when it gives one.
    pub detail: Option<String>,
    /// The CLI's own rule content for "always", when it offers one, such
    /// as `domain:example.com`.
    pub suggestion: Option<String>,
}

/// The reader's answer, as the agent is given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny { message: String },
}

impl Decision {
    pub fn deny(message: impl Into<String>) -> Self {
        Decision::Deny {
            message: message.into(),
        }
    }
}

/// What a line of output asks of the pump.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Reply {
    /// Lines to write to stdin, each without its newline.
    pub writes: Vec<String>,
    /// Events beyond what the backend's parser makes of the line.
    pub events: Vec<RunEvent>,
    /// Close stdin: the CLI is done and exits on EOF.
    pub close: bool,
}

pub(crate) enum Conversation {
    Claude(ClaudeHost),
    Codex(Box<CodexServer>),
}

impl Conversation {
    /// What to write as the process starts.
    pub fn opening(&mut self) -> Vec<String> {
        match self {
            Conversation::Claude(c) => c.opening(),
            Conversation::Codex(c) => c.opening(),
        }
    }

    pub fn after_line(&mut self, value: &Value) -> Reply {
        match self {
            Conversation::Claude(c) => c.after_line(value),
            Conversation::Codex(c) => c.after_line(value),
        }
    }

    /// The line that answers request `id`, or `None` for a request that
    /// is not pending, already answered or never made.
    pub fn answer(&mut self, id: &str, decision: &Decision) -> Option<String> {
        match self {
            Conversation::Claude(c) => c.answer(id, decision),
            Conversation::Codex(c) => c.answer(id, decision),
        }
    }

    /// Whether this conversation's lines are parsed by it alone, rather
    /// than by the backend's print-mode parser first.
    pub fn parses_everything(&self) -> bool {
        matches!(self, Conversation::Codex(_))
    }
}

// ---------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------

/// Claude Code with `--input-format stream-json` and
/// `--permission-prompt-tool stdio`. Its output is the same stream-json
/// the print parser reads; this adds the control requests, and closes
/// stdin after the result so the process exits.
pub(crate) struct ClaudeHost {
    prompt: String,
    /// Each pending request's input, sent back as `updatedInput` when
    /// it is allowed, since the CLI takes the input to run from the
    /// answer.
    pending: HashMap<String, Value>,
}

impl ClaudeHost {
    pub fn new(prompt: String) -> Self {
        Self {
            prompt,
            pending: HashMap::new(),
        }
    }

    fn opening(&mut self) -> Vec<String> {
        vec![
            json!({
                "type": "control_request",
                "request_id": "perch-initialize",
                "request": {"subtype": "initialize"},
            })
            .to_string(),
            json!({
                "type": "user",
                "message": {"role": "user", "content": self.prompt},
            })
            .to_string(),
        ]
    }

    fn after_line(&mut self, v: &Value) -> Reply {
        match v.get("type").and_then(Value::as_str) {
            Some("control_request") => {
                let id = str_of(v, "request_id");
                let request = v.get("request").cloned().unwrap_or(Value::Null);
                if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
                    // Hook callbacks and MCP messages: Perch registers
                    // neither, so any that arrive are refused rather than
                    // left to hang the run.
                    return Reply {
                        writes: vec![json!({
                            "type": "control_response",
                            "response": {
                                "subtype": "error",
                                "request_id": id,
                                "error": "Perch does not handle this request",
                            },
                        })
                        .to_string()],
                        ..Reply::default()
                    };
                }
                let input = request.get("input").cloned().unwrap_or(Value::Null);
                self.pending.insert(id.clone(), input);
                Reply {
                    events: vec![RunEvent::Approval {
                        id,
                        request: claude_request(&request),
                    }],
                    ..Reply::default()
                }
            }
            Some("result") => Reply {
                close: true,
                ..Reply::default()
            },
            _ => Reply::default(),
        }
    }

    fn answer(&mut self, id: &str, decision: &Decision) -> Option<String> {
        let input = self.pending.remove(id)?;
        let response = match decision {
            Decision::Allow => json!({"behavior": "allow", "updatedInput": input}),
            Decision::Deny { message } => json!({"behavior": "deny", "message": message}),
        };
        Some(
            json!({
                "type": "control_response",
                "response": {"subtype": "success", "request_id": id, "response": response},
            })
            .to_string(),
        )
    }
}

/// A `can_use_tool` request as Perch shows it.
fn claude_request(request: &Value) -> ApprovalRequest {
    let tool = str_of(request, "tool_name");
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    let description = str_of(request, "description");
    let detail = (!description.trim().is_empty()).then(|| one_line(&description, 200));
    let suggestion = request
        .pointer("/permission_suggestions/0/rules/0/ruleContent")
        .and_then(Value::as_str)
        .map(str::to_string);
    match tool.as_str() {
        "SandboxNetworkAccess" => ApprovalRequest {
            kind: ApprovalKind::Host,
            subject: str_of(&input, "host"),
            detail: None,
            suggestion: None,
        },
        "Bash" => ApprovalRequest {
            kind: ApprovalKind::Command,
            subject: str_of(&input, "command"),
            detail,
            suggestion: None,
        },
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let path = [str_of(&input, "file_path"), str_of(&input, "notebook_path")]
                .into_iter()
                .find(|p| !p.is_empty())
                .unwrap_or_default();
            ApprovalRequest {
                kind: ApprovalKind::Edit,
                subject: path,
                detail: None,
                suggestion: None,
            }
        }
        _ => ApprovalRequest {
            subject: summarize_tool_call(&tool, &input),
            kind: ApprovalKind::Tool { name: tool },
            detail,
            suggestion,
        },
    }
}

// ---------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------

/// What a Codex thread is started with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CodexThread {
    pub cwd: String,
    pub model: String,
    pub effort: String,
    pub prompt: String,
    pub developer_instructions: Option<String>,
    /// The thread to resume, for a follow-up.
    pub resume: Option<String>,
    /// Config for the thread: the Work profile, trust, and the like.
    pub config: Value,
}

const INITIALIZE: u64 = 1;
const THREAD: u64 = 2;
const TURN: u64 = 3;

/// `codex app-server` over stdio. Perch starts or resumes one thread,
/// starts one turn in it, turns the item notifications into the steps
/// `codex exec` gave, and closes stdin when the turn completes, which
/// ends the server.
pub(crate) struct CodexServer {
    thread: CodexThread,
    thread_id: String,
    /// Commands that have begun, so a completion never marks the wrong
    /// step, as in `CodexParser`.
    started: std::collections::HashSet<String>,
    /// Each pending approval by its request id as Perch names it, with
    /// the JSON-RPC id to answer and the method it came by.
    pending: HashMap<String, (Value, String, Value)>,
    errors: usize,
    done: bool,
}

impl CodexServer {
    pub fn new(thread: CodexThread) -> Self {
        Self {
            thread,
            thread_id: String::new(),
            started: Default::default(),
            pending: HashMap::new(),
            errors: 0,
            done: false,
        }
    }

    #[cfg(test)]
    pub fn thread_for_tests(&self) -> CodexThread {
        self.thread.clone()
    }

    fn opening(&mut self) -> Vec<String> {
        vec![request(
            INITIALIZE,
            "initialize",
            json!({"clientInfo": {"name": "perch", "version": env!("CARGO_PKG_VERSION")}}),
        )]
    }

    fn thread_params(&self) -> Value {
        let mut params = json!({
            "cwd": self.thread.cwd,
            "model": self.thread.model,
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "config": self.thread.config,
        });
        if let Some(instructions) = &self.thread.developer_instructions {
            params["developerInstructions"] = json!(instructions);
        }
        if let Some(id) = &self.thread.resume {
            params["threadId"] = json!(id);
        }
        params
    }

    fn after_line(&mut self, v: &Value) -> Reply {
        if self.done {
            return Reply::default();
        }
        // A response to one of Perch's requests.
        if let Some(id) = v.get("id").and_then(Value::as_u64).filter(|_| v.get("method").is_none()) {
            if let Some(error) = v.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex refused the request")
                    .to_string();
                return self.fail(message);
            }
            let result = v.get("result").cloned().unwrap_or(Value::Null);
            return match id {
                INITIALIZE => {
                    let method = if self.thread.resume.is_some() {
                        "thread/resume"
                    } else {
                        "thread/start"
                    };
                    Reply {
                        writes: vec![
                            json!({"jsonrpc": "2.0", "method": "initialized"}).to_string(),
                            request(THREAD, method, self.thread_params()),
                        ],
                        ..Reply::default()
                    }
                }
                THREAD => {
                    self.thread_id = result
                        .pointer("/thread/id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| self.thread.resume.clone())
                        .unwrap_or_default();
                    Reply {
                        writes: vec![request(
                            TURN,
                            "turn/start",
                            json!({
                                "threadId": self.thread_id,
                                "input": [{"type": "text", "text": self.thread.prompt}],
                                "effort": self.thread.effort,
                            }),
                        )],
                        events: vec![RunEvent::Started {
                            session_id: self.thread_id.clone(),
                            model: self.thread.model.clone(),
                        }],
                        ..Reply::default()
                    }
                }
                _ => Reply::default(),
            };
        }
        let method = v.get("method").and_then(Value::as_str).unwrap_or("");
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        // A request from the server.
        if let Some(id) = v.get("id").cloned() {
            return self.server_request(id, method, params);
        }
        match method {
            "item/started" => self.item_started(params.get("item").unwrap_or(&Value::Null)),
            "item/completed" => self.item_completed(params.get("item").unwrap_or(&Value::Null)),
            "thread/tokenUsage/updated" => {
                let total = params.pointer("/tokenUsage/total").cloned().unwrap_or(Value::Null);
                Reply {
                    events: vec![RunEvent::Usage {
                        input_tokens: total.get("inputTokens").and_then(Value::as_u64),
                        output_tokens: total.get("outputTokens").and_then(Value::as_u64),
                        cached_input_tokens: total.get("cachedInputTokens").and_then(Value::as_u64),
                    }],
                    ..Reply::default()
                }
            }
            "error" => {
                // A retried error is a step; the turn goes on.
                self.errors += 1;
                let raw = params
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Reply {
                    events: failed_step(format!("error_{}", self.errors), &human_message(raw)),
                    ..Reply::default()
                }
            }
            "turn/completed" => {
                let turn = params.get("turn").cloned().unwrap_or(Value::Null);
                if turn.get("status").and_then(Value::as_str) == Some("failed") {
                    let raw = turn
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let message = human_message(raw);
                    return self.fail(if message.is_empty() {
                        "The turn failed".to_string()
                    } else {
                        message
                    });
                }
                self.done = true;
                Reply {
                    events: vec![RunEvent::Finished {
                        session_id: self.thread_id.clone(),
                        cost_usd: None,
                        duration_ms: 0,
                    }],
                    close: true,
                    ..Reply::default()
                }
            }
            _ => Reply::default(),
        }
    }

    fn fail(&mut self, message: String) -> Reply {
        self.done = true;
        Reply {
            events: vec![RunEvent::Failed { message }],
            close: true,
            ..Reply::default()
        }
    }

    fn server_request(&mut self, id: Value, method: &str, params: Value) -> Reply {
        let request = match method {
            "item/commandExecution/requestApproval" => {
                let host = params
                    .pointer("/networkApprovalContext/host")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let reason = str_of(&params, "reason");
                let detail = (!reason.trim().is_empty()).then(|| one_line(&reason, 200));
                match host {
                    Some(host) => ApprovalRequest {
                        kind: ApprovalKind::Host,
                        subject: host,
                        detail,
                        suggestion: None,
                    },
                    None => ApprovalRequest {
                        kind: ApprovalKind::Command,
                        subject: strip_shell(&str_of(&params, "command")),
                        detail,
                        suggestion: None,
                    },
                }
            }
            "item/fileChange/requestApproval" => {
                let reason = str_of(&params, "reason");
                let root = str_of(&params, "grantRoot");
                ApprovalRequest {
                    kind: ApprovalKind::Edit,
                    subject: if root.is_empty() {
                        "files outside the worktree".to_string()
                    } else {
                        root
                    },
                    detail: (!reason.trim().is_empty()).then(|| one_line(&reason, 200)),
                    suggestion: None,
                }
            }
            "item/permissions/requestApproval" => {
                let reason = str_of(&params, "reason");
                ApprovalRequest {
                    kind: ApprovalKind::Tool {
                        name: "permissions".into(),
                    },
                    subject: permissions_summary(params.get("permissions").unwrap_or(&Value::Null)),
                    detail: (!reason.trim().is_empty()).then(|| one_line(&reason, 200)),
                    suggestion: None,
                }
            }
            _ => {
                // Anything else Perch does not answer: an error, so the
                // server goes on rather than waiting.
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
        };
        let key = match &id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        self.pending.insert(key.clone(), (id, method.to_string(), params));
        Reply {
            events: vec![RunEvent::Approval { id: key, request }],
            ..Reply::default()
        }
    }

    fn answer(&mut self, id: &str, decision: &Decision) -> Option<String> {
        let (rpc_id, method, params) = self.pending.remove(id)?;
        let allow = matches!(decision, Decision::Allow);
        let result = if method == "item/permissions/requestApproval" {
            if allow {
                json!({"permissions": params.get("permissions").cloned().unwrap_or(json!({})), "scope": "turn"})
            } else {
                json!({"permissions": {}})
            }
        } else {
            json!({"decision": if allow { "accept" } else { "decline" }})
        };
        Some(json!({"jsonrpc": "2.0", "id": rpc_id, "result": result}).to_string())
    }

    fn item_started(&mut self, item: &Value) -> Reply {
        if str_of(item, "type") != "commandExecution" {
            return Reply::default();
        }
        let id = str_of(item, "id");
        self.started.insert(id.clone());
        Reply {
            events: vec![command_call(id, item)],
            ..Reply::default()
        }
    }

    fn item_completed(&mut self, item: &Value) -> Reply {
        let id = str_of(item, "id");
        let events = match str_of(item, "type").as_str() {
            "commandExecution" => {
                let ok = item.get("exitCode").and_then(Value::as_i64) == Some(0);
                let summary = if ok {
                    String::new()
                } else {
                    last_line(&str_of(item, "aggregatedOutput"))
                };
                let result = RunEvent::ToolResult {
                    id: id.clone(),
                    ok,
                    summary,
                };
                if self.started.remove(&id) {
                    vec![result]
                } else {
                    vec![command_call(id, item), result]
                }
            }
            "fileChange" => {
                let paths: Vec<String> = item
                    .get("changes")
                    .and_then(Value::as_array)
                    .map(|changes| changes.iter().map(|c| str_of(c, "path")).collect())
                    .unwrap_or_default();
                let names: Vec<&str> = paths
                    .iter()
                    .map(|p| p.rsplit('/').next().unwrap_or(p))
                    .collect();
                let ok = str_of(item, "status") != "failed";
                vec![
                    RunEvent::ToolCall {
                        id: id.clone(),
                        name: "file_change".into(),
                        summary: one_line(&format!("Edited {}", names.join(", ")), 120),
                        parent_id: None,
                    },
                    RunEvent::ToolResult {
                        id,
                        ok,
                        summary: String::new(),
                    },
                ]
            }
            "agentMessage" => {
                let text = str_of(item, "text");
                if text.trim().is_empty() {
                    Vec::new()
                } else {
                    vec![RunEvent::Text(text)]
                }
            }
            _ => Vec::new(),
        };
        Reply {
            events,
            ..Reply::default()
        }
    }
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn command_call(id: String, item: &Value) -> RunEvent {
    let command = strip_shell(&str_of(item, "command"));
    RunEvent::ToolCall {
        id,
        name: "command_execution".into(),
        summary: one_line(&format!("Ran {command}"), 120),
        parent_id: None,
    }
}

fn failed_step(id: String, message: &str) -> Vec<RunEvent> {
    vec![
        RunEvent::ToolCall {
            id: id.clone(),
            name: "error".into(),
            summary: one_line(message, 120),
            parent_id: None,
        },
        RunEvent::ToolResult {
            id,
            ok: false,
            summary: String::new(),
        },
    ]
}

/// What a Codex permissions request asks for, in a line.
fn permissions_summary(permissions: &Value) -> String {
    let mut parts = Vec::new();
    if permissions.pointer("/network/enabled").and_then(Value::as_bool) == Some(true) {
        parts.push("network access".to_string());
    }
    let writes: Vec<String> = permissions
        .pointer("/fileSystem/write")
        .and_then(Value::as_array)
        .map(|paths| paths.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if !writes.is_empty() {
        parts.push(format!("writing {}", writes.join(", ")));
    }
    if parts.is_empty() {
        "more permissions".to_string()
    } else {
        parts.join(" and ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn claude_opens_with_initialize_and_the_prompt() {
        let mut host = ClaudeHost::new("Do it".into());
        let opening = host.opening();
        assert_eq!(opening.len(), 2);
        assert!(opening[0].contains("\"subtype\":\"initialize\""));
        let user: Value = serde_json::from_str(&opening[1]).unwrap();
        assert_eq!(user["message"]["content"], "Do it");
    }

    #[test]
    fn claude_requests_are_what_the_card_shows_and_answers_what_the_cli_takes() {
        let mut host = ClaudeHost::new(String::new());
        let network = host.after_line(&line(
            r#"{"type":"control_request","request_id":"n1","request":{"subtype":"can_use_tool","tool_name":"SandboxNetworkAccess","input":{"host":"example.com"},"description":"Allow network connection to example.com?"}}"#,
        ));
        assert_eq!(
            network.events,
            vec![RunEvent::Approval {
                id: "n1".into(),
                request: ApprovalRequest {
                    kind: ApprovalKind::Host,
                    subject: "example.com".into(),
                    detail: None,
                    suggestion: None,
                },
            }]
        );
        let command = host.after_line(&line(
            r#"{"type":"control_request","request_id":"c1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"git commit -m x","dangerouslyDisableSandbox":true},"description":"Commit the change"}}"#,
        ));
        assert!(matches!(
            &command.events[0],
            RunEvent::Approval { request: ApprovalRequest { kind: ApprovalKind::Command, subject, detail: Some(d), .. }, .. }
                if subject == "git commit -m x" && d == "Commit the change"
        ));
        let edit = host.after_line(&line(
            r#"{"type":"control_request","request_id":"e1","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"/Users/me/.zshrc","content":"x"}}}"#,
        ));
        assert!(matches!(
            &edit.events[0],
            RunEvent::Approval { request: ApprovalRequest { kind: ApprovalKind::Edit, subject, .. }, .. } if subject == "/Users/me/.zshrc"
        ));

        let allowed: Value = serde_json::from_str(&host.answer("c1", &Decision::Allow).unwrap()).unwrap();
        assert_eq!(allowed["response"]["request_id"], "c1");
        assert_eq!(allowed["response"]["response"]["behavior"], "allow");
        assert_eq!(
            allowed["response"]["response"]["updatedInput"]["command"],
            "git commit -m x",
            "the input goes back as it came"
        );
        assert_eq!(host.answer("c1", &Decision::Allow), None, "answered once");
        let denied: Value =
            serde_json::from_str(&host.answer("n1", &Decision::deny("The reader said no")).unwrap()).unwrap();
        assert_eq!(denied["response"]["response"]["behavior"], "deny");
        assert_eq!(denied["response"]["response"]["message"], "The reader said no");
    }

    #[test]
    fn claude_other_control_requests_are_refused_and_the_result_closes_stdin() {
        let mut host = ClaudeHost::new(String::new());
        let hook = host.after_line(&line(
            r#"{"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback"}}"#,
        ));
        assert!(hook.events.is_empty());
        assert!(hook.writes[0].contains("\"subtype\":\"error\""));
        assert!(host.after_line(&line(r#"{"type":"result","subtype":"success"}"#)).close);
    }

    fn thread(resume: Option<&str>) -> CodexThread {
        CodexThread {
            cwd: "/wt".into(),
            model: "gpt".into(),
            effort: "high".into(),
            prompt: "Do it".into(),
            developer_instructions: Some("Work here.".into()),
            resume: resume.map(str::to_string),
            config: json!({"default_permissions": "perch_work"}),
        }
    }

    #[test]
    fn codex_initializes_starts_a_thread_and_a_turn() {
        let mut server = CodexServer::new(thread(None));
        let opening = server.opening();
        assert!(opening[0].contains("\"method\":\"initialize\""));
        let after_init = server.after_line(&line(r#"{"id":1,"result":{"userAgent":"x"}}"#));
        assert!(after_init.writes[0].contains("\"method\":\"initialized\""));
        let start: Value = serde_json::from_str(&after_init.writes[1]).unwrap();
        assert_eq!(start["method"], "thread/start");
        assert_eq!(start["params"]["approvalsReviewer"], "user");
        assert_eq!(start["params"]["approvalPolicy"], "on-request");
        assert_eq!(start["params"]["developerInstructions"], "Work here.");
        assert_eq!(start["params"]["config"]["default_permissions"], "perch_work");
        let after_thread = server.after_line(&line(r#"{"id":2,"result":{"thread":{"id":"th-1"}}}"#));
        let turn: Value = serde_json::from_str(&after_thread.writes[0]).unwrap();
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["threadId"], "th-1");
        assert_eq!(turn["params"]["input"][0]["text"], "Do it");
        assert_eq!(
            after_thread.events,
            vec![RunEvent::Started { session_id: "th-1".into(), model: "gpt".into() }]
        );

        let mut resumed = CodexServer::new(thread(Some("th-0")));
        resumed.opening();
        let start: Value =
            serde_json::from_str(&resumed.after_line(&line(r#"{"id":1,"result":{}}"#)).writes[1]).unwrap();
        assert_eq!(start["method"], "thread/resume");
        assert_eq!(start["params"]["threadId"], "th-0");
    }

    #[test]
    fn codex_items_become_steps_and_the_turn_ends_the_run() {
        let mut server = CodexServer::new(thread(None));
        server.after_line(&line(r#"{"id":2,"result":{"thread":{"id":"th-1"}}}"#));
        let started = server.after_line(&line(
            r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"c1","command":"/bin/zsh -lc ls"}}}"#,
        ));
        assert!(matches!(&started.events[0], RunEvent::ToolCall { summary, .. } if summary == "Ran ls"));
        let done = server.after_line(&line(
            r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"c1","command":"/bin/zsh -lc ls","exitCode":1,"aggregatedOutput":"no such file\n"}}}"#,
        ));
        assert_eq!(
            done.events,
            vec![RunEvent::ToolResult { id: "c1".into(), ok: false, summary: "no such file".into() }]
        );
        let edit = server.after_line(&line(
            r#"{"method":"item/completed","params":{"item":{"type":"fileChange","id":"f1","changes":[{"path":"/wt/notes.md"}],"status":"completed"}}}"#,
        ));
        assert!(matches!(&edit.events[0], RunEvent::ToolCall { summary, .. } if summary == "Edited notes.md"));
        let said = server.after_line(&line(
            r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m1","text":"Done."}}}"#,
        ));
        assert_eq!(said.events, vec![RunEvent::Text("Done.".into())]);
        let usage = server.after_line(&line(
            r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":10,"outputTokens":2,"cachedInputTokens":1}}}}"#,
        ));
        assert_eq!(
            usage.events,
            vec![RunEvent::Usage { input_tokens: Some(10), output_tokens: Some(2), cached_input_tokens: Some(1) }]
        );
        let end = server.after_line(&line(r#"{"method":"turn/completed","params":{"turn":{"status":"completed"}}}"#));
        assert!(end.close);
        assert!(matches!(&end.events[0], RunEvent::Finished { session_id, .. } if session_id == "th-1"));
    }

    #[test]
    fn codex_approvals_are_asked_and_answered_accept_or_decline() {
        let mut server = CodexServer::new(thread(None));
        let network = server.after_line(&line(
            r#"{"method":"item/commandExecution/requestApproval","id":7,"params":{"command":"/bin/zsh -lc \"curl https://example.com\"","reason":"needs the network","networkApprovalContext":{"host":"example.com","protocol":"https"}}}"#,
        ));
        assert_eq!(
            network.events,
            vec![RunEvent::Approval {
                id: "7".into(),
                request: ApprovalRequest {
                    kind: ApprovalKind::Host,
                    subject: "example.com".into(),
                    detail: Some("needs the network".into()),
                    suggestion: None,
                },
            }]
        );
        let command = server.after_line(&line(
            r#"{"method":"item/commandExecution/requestApproval","id":"x8","params":{"command":"/bin/zsh -lc 'echo hi > /Users/me/f'"}}"#,
        ));
        assert!(matches!(
            &command.events[0],
            RunEvent::Approval { id, request: ApprovalRequest { kind: ApprovalKind::Command, subject, .. } }
                if id == "x8" && subject == "echo hi > /Users/me/f"
        ));
        let accept: Value = serde_json::from_str(&server.answer("7", &Decision::Allow).unwrap()).unwrap();
        assert_eq!(accept["id"], 7, "the server's id, as it sent it");
        assert_eq!(accept["result"]["decision"], "accept");
        let decline: Value = serde_json::from_str(&server.answer("x8", &Decision::deny("no")).unwrap()).unwrap();
        assert_eq!(decline["id"], "x8");
        assert_eq!(decline["result"]["decision"], "decline");

        let other = server.after_line(&line(r#"{"method":"item/tool/call","id":9,"params":{}}"#));
        assert!(other.events.is_empty());
        assert!(other.writes[0].contains("\"error\""));
    }

    #[test]
    fn codex_failures_end_the_run() {
        let mut server = CodexServer::new(thread(None));
        let refused = server.after_line(&line(r#"{"id":2,"error":{"code":1,"message":"no such model"}}"#));
        assert!(refused.close);
        assert_eq!(refused.events, vec![RunEvent::Failed { message: "no such model".into() }]);

        let mut server = CodexServer::new(thread(None));
        let failed = server.after_line(&line(
            r#"{"method":"turn/completed","params":{"turn":{"status":"failed","error":{"message":"quota"}}}}"#,
        ));
        assert_eq!(failed.events, vec![RunEvent::Failed { message: "quota".into() }]);
    }
}
