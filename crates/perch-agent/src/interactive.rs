//! Conversations with a CLI over its stdin, for every run but a review.
//!
//! A Work run started in print mode can only be refused at its sandbox's
//! edge. Started this way it asks, and Perch answers. A question never
//! asks, but is held the same way so that it can be steered. Claude Code is
//! driven by its host permission protocol: stream-json in and out, with
//! `can_use_tool` control requests for anything its permissions would
//! otherwise prompt for. Codex is driven through `codex app-server`, a
//! JSON-RPC peer that starts a thread and a turn on request and sends
//! its approval requests to the client. Gemini's Work goes through
//! `agy acp`, another such peer, and its questions through print mode on
//! stdin; see `gemini`.
//!
//! A conversation is told each line the CLI prints. It says what to write
//! back, which events the line makes, and when stdin should close, which
//! is how both CLIs are told there is nothing more to do.
//!
//! The reader can also steer a run: say something more while it works.
//! Claude Code reads a user message that comes mid-turn once its current
//! tool call returns; Codex takes one by `turn/steer`. Each CLI says where
//! the message landed, and stdin stays open until every one has. One the
//! turn ended before reading starts another turn in the same run.

use std::collections::{HashMap, VecDeque};

use serde_json::{json, Value};

use crate::codex::{command_call, failed_step, human_message, last_line, strip_shell};
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
    /// The line ends a turn but not the run: a steer the turn ended
    /// before reading makes another. Its `Finished` is not the run's.
    pub more: bool,
}

pub(crate) enum Conversation {
    Claude(ClaudeHost),
    Codex(Box<CodexServer>),
    /// Gemini in print mode, for a question.
    Gemini(crate::gemini::GeminiHost),
    /// Gemini as `agy acp`, for Work.
    Acp(Box<crate::gemini::AcpClient>),
}

impl Conversation {
    /// What to write as the process starts.
    pub fn opening(&mut self) -> Vec<String> {
        match self {
            Conversation::Claude(c) => c.opening(),
            Conversation::Codex(c) => c.opening(),
            Conversation::Gemini(c) => c.opening(),
            Conversation::Acp(c) => c.opening(),
        }
    }

    pub fn after_line(&mut self, value: &Value) -> Reply {
        match self {
            Conversation::Claude(c) => c.after_line(value),
            Conversation::Codex(c) => c.after_line(value),
            Conversation::Gemini(c) => c.after_line(value),
            Conversation::Acp(c) => c.after_line(value),
        }
    }

    /// The line that answers request `id`, or `None` for a request that
    /// is not pending, already answered or never made.
    pub fn answer(&mut self, id: &str, decision: &Decision) -> Option<String> {
        match self {
            Conversation::Claude(c) => c.answer(id, decision),
            Conversation::Codex(c) => c.answer(id, decision),
            // A question in print mode never asks.
            Conversation::Gemini(_) => None,
            Conversation::Acp(c) => c.answer(id, decision),
        }
    }

    /// The lines that give the agent a message from the reader while it
    /// works. It lands as a `RunEvent::Steered` when the agent reads it.
    pub fn steer(&mut self, text: &str) -> Vec<String> {
        match self {
            Conversation::Claude(c) => c.steer(text),
            Conversation::Codex(c) => c.steer(text),
            Conversation::Gemini(c) => c.steer(text),
            Conversation::Acp(c) => c.steer(text),
        }
    }

    /// Whether this conversation's lines are parsed by it alone, rather
    /// than by the backend's print-mode parser first.
    pub fn parses_everything(&self) -> bool {
        matches!(self, Conversation::Codex(_) | Conversation::Acp(_))
    }
}

// ---------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------

/// Claude Code with `--input-format stream-json` and
/// `--replay-user-messages`, and for Work `--permission-prompt-tool
/// stdio`. Its output is the same stream-json the print parser reads;
/// this adds the control requests and the reader's steers, and closes
/// stdin after the last result so the process exits.
pub(crate) struct ClaudeHost {
    prompt: String,
    /// Each pending request's input, sent back as `updatedInput` when
    /// it is allowed, since the CLI takes the input to run from the
    /// answer.
    pending: HashMap<String, Value>,
    /// The reader's messages the agent has not read yet, oldest first.
    /// Claude Code replays each as it reads it.
    steers: VecDeque<String>,
}

impl ClaudeHost {
    pub fn new(prompt: String) -> Self {
        Self {
            prompt,
            pending: HashMap::new(),
            steers: VecDeque::new(),
        }
    }

    fn steer(&mut self, text: &str) -> Vec<String> {
        self.steers.push_back(text.to_string());
        vec![claude_user(text)]
    }

    fn opening(&mut self) -> Vec<String> {
        vec![
            json!({
                "type": "control_request",
                "request_id": "perch-initialize",
                "request": {"subtype": "initialize"},
            })
            .to_string(),
            claude_user(&self.prompt),
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
            // A replayed message is one the agent has just read.
            // Subagents' prompts and tool results are not replays.
            Some("user")
                if v.get("isReplay").and_then(Value::as_bool) == Some(true)
                    && v.get("parent_tool_use_id").is_none_or(Value::is_null) =>
            {
                let content = v.pointer("/message/content").and_then(Value::as_str);
                match (content, self.steers.front()) {
                    (Some(content), Some(front)) if content == front => {
                        let text = self.steers.pop_front().unwrap_or_default();
                        Reply {
                            events: vec![RunEvent::Steered(text)],
                            ..Reply::default()
                        }
                    }
                    _ => Reply::default(),
                }
            }
            // A steer the turn ended before reading is the next turn's
            // prompt: Claude Code starts it on its own, and it ends in a
            // result of its own. A failed turn ends the run regardless.
            Some("result") => {
                let failed = v.get("is_error").and_then(Value::as_bool) == Some(true)
                    || v.get("subtype")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s != "success");
                if self.steers.is_empty() || failed {
                    Reply {
                        close: true,
                        ..Reply::default()
                    }
                } else {
                    Reply {
                        more: true,
                        ..Reply::default()
                    }
                }
            }
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

fn claude_user(text: &str) -> String {
    json!({
        "type": "user",
        "message": {"role": "user", "content": text},
    })
    .to_string()
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
    /// `on-request` for Work, which asks; `never` for a question, which
    /// is refused at its read-only sandbox's edge instead.
    pub approval_policy: String,
    /// Who approval requests go to: `user`, which is Perch, or Codex's
    /// own `auto_review`. `None` for a thread that never asks.
    pub reviewer: Option<String>,
    /// The sandbox mode, for a question: `read-only`. Work's sandbox is
    /// its permission profile, which Codex will not take beside a mode.
    pub sandbox: Option<String>,
}

const INITIALIZE: u64 = 1;
const THREAD: u64 = 2;
const TURN: u64 = 3;
const UNARCHIVE: u64 = 4;
const ARCHIVE: u64 = 5;
/// Perch's later requests count up from here: steers, and the turns that
/// carry a steer the last turn ended before reading.
const LATER: u64 = 100;

/// `codex app-server` over stdio. Perch starts or resumes one thread,
/// starts a turn in it, turns the item notifications into the steps
/// `codex exec` gave, and closes stdin when the last turn completes,
/// which ends the server.
///
/// Codex records the thread as an interactive session, which the Codex
/// app would list beside the reader's own. So the thread is archived as
/// the run ends, and unarchived before it is resumed, since Codex will
/// not resume an archived thread.
pub(crate) struct CodexServer {
    thread: CodexThread,
    thread_id: String,
    /// The turn going now, once Codex has named it. A steer names it.
    turn: Option<String>,
    /// The reader's messages the agent has not read yet, oldest first.
    steers: VecDeque<String>,
    /// How many of `steers`, from the front, have gone to Codex.
    sent: usize,
    next_id: u64,
    /// The ids of the later `turn/start` requests.
    later_turns: std::collections::HashSet<u64>,
    /// Commands that have begun, so a completion never marks the wrong
    /// step, as in `CodexParser`.
    started: std::collections::HashSet<String>,
    /// Each pending approval by its request id as Perch names it, with
    /// the JSON-RPC id to answer and the method it came by.
    pending: HashMap<String, (Value, String, Value)>,
    errors: usize,
    done: bool,
    /// The run is over and waits on `thread/archive` to close stdin.
    archiving: bool,
}

impl CodexServer {
    pub fn new(thread: CodexThread) -> Self {
        Self {
            thread,
            thread_id: String::new(),
            turn: None,
            steers: VecDeque::new(),
            sent: 0,
            next_id: LATER,
            later_turns: Default::default(),
            started: Default::default(),
            pending: HashMap::new(),
            errors: 0,
            done: false,
            archiving: false,
        }
    }

    fn later_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn steer(&mut self, text: &str) -> Vec<String> {
        if self.done {
            return Vec::new();
        }
        self.steers.push_back(text.to_string());
        // Before Codex names the turn, the steer waits for it.
        self.send_steers()
    }

    /// `turn/steer` for each steer not yet sent, when a turn is going.
    fn send_steers(&mut self) -> Vec<String> {
        let Some(turn) = self.turn.clone() else {
            return Vec::new();
        };
        let unsent: Vec<String> = self.steers.iter().skip(self.sent).cloned().collect();
        self.sent = self.steers.len();
        unsent
            .into_iter()
            .map(|text| {
                let id = self.later_id();
                request(
                    id,
                    "turn/steer",
                    json!({
                        "threadId": self.thread_id,
                        "expectedTurnId": turn,
                        "input": [{"type": "text", "text": text}],
                    }),
                )
            })
            .collect()
    }

    fn start_turn(&mut self, id: u64, text: &str) -> String {
        request(
            id,
            "turn/start",
            json!({
                "threadId": self.thread_id,
                "input": [{"type": "text", "text": text}],
                "effort": self.thread.effort,
            }),
        )
    }

    fn finish(&mut self) -> Reply {
        let session_id = self.thread_id.clone();
        self.end(RunEvent::Finished {
            session_id,
            cost_usd: None,
            duration_ms: 0,
        })
    }

    fn fail(&mut self, message: String) -> Reply {
        self.end(RunEvent::Failed { message })
    }

    /// The run's last event, and `thread/archive` for its thread, if it
    /// has one. Stdin closes once the archive is answered, either way.
    fn end(&mut self, event: RunEvent) -> Reply {
        self.done = true;
        let thread = Some(self.thread_id.clone())
            .filter(|id| !id.is_empty())
            .or_else(|| self.thread.resume.clone());
        let Some(thread) = thread else {
            return Reply {
                events: vec![event],
                close: true,
                ..Reply::default()
            };
        };
        self.archiving = true;
        Reply {
            writes: vec![request(
                ARCHIVE,
                "thread/archive",
                json!({"threadId": thread}),
            )],
            events: vec![event],
            ..Reply::default()
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
            "approvalPolicy": self.thread.approval_policy,
            "config": self.thread.config,
        });
        if let Some(reviewer) = &self.thread.reviewer {
            params["approvalsReviewer"] = json!(reviewer);
        }
        if let Some(sandbox) = &self.thread.sandbox {
            params["sandbox"] = json!(sandbox);
        }
        if let Some(instructions) = &self.thread.developer_instructions {
            params["developerInstructions"] = json!(instructions);
        }
        if let Some(id) = &self.thread.resume {
            params["threadId"] = json!(id);
        }
        params
    }

    fn after_line(&mut self, v: &Value) -> Reply {
        let response = v
            .get("id")
            .and_then(Value::as_u64)
            .filter(|_| v.get("method").is_none());
        if self.done {
            if self.archiving && response == Some(ARCHIVE) {
                self.archiving = false;
                return Reply {
                    close: true,
                    ..Reply::default()
                };
            }
            return Reply::default();
        }
        // A response to one of Perch's requests.
        if let Some(id) = response {
            // A thread that was never archived refuses to be unarchived;
            // the resume goes ahead either way.
            if id == UNARCHIVE {
                return Reply {
                    writes: vec![request(THREAD, "thread/resume", self.thread_params())],
                    ..Reply::default()
                };
            }
            if let Some(error) = v.get("error") {
                if id >= LATER {
                    // A steer that missed its turn goes in the next one,
                    // which the turn's completion starts. A later turn
                    // Codex will not start leaves nothing going.
                    return if self.later_turns.contains(&id) {
                        self.finish()
                    } else {
                        Reply::default()
                    };
                }
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
                    let next = match &self.thread.resume {
                        Some(id) => request(UNARCHIVE, "thread/unarchive", json!({"threadId": id})),
                        None => request(THREAD, "thread/start", self.thread_params()),
                    };
                    Reply {
                        writes: vec![
                            json!({"jsonrpc": "2.0", "method": "initialized"}).to_string(),
                            next,
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
                    let prompt = self.thread.prompt.clone();
                    Reply {
                        writes: vec![self.start_turn(TURN, &prompt)],
                        events: vec![RunEvent::Started {
                            session_id: self.thread_id.clone(),
                            model: self.thread.model.clone(),
                        }],
                        ..Reply::default()
                    }
                }
                id if id == TURN || self.later_turns.contains(&id) => {
                    self.turn = result
                        .pointer("/turn/id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    Reply {
                        writes: self.send_steers(),
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
            "turn/started" => {
                if let Some(turn) = params.pointer("/turn/id").and_then(Value::as_str) {
                    self.turn = Some(turn.to_string());
                }
                Reply {
                    writes: self.send_steers(),
                    ..Reply::default()
                }
            }
            "item/started" => self.item_started(params.get("item").unwrap_or(&Value::Null)),
            "item/completed" => self.item_completed(params.get("item").unwrap_or(&Value::Null)),
            "thread/tokenUsage/updated" => {
                let total = params
                    .pointer("/tokenUsage/total")
                    .cloned()
                    .unwrap_or(Value::Null);
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
            "item/autoApprovalReview/completed" => Reply {
                events: auto_review_step(&params),
                ..Reply::default()
            },
            "turn/completed" => {
                self.turn = None;
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
                // A steer the turn ended before reading starts the next
                // one; any others follow it by `turn/steer`.
                if let Some(text) = self.steers.front().cloned() {
                    let id = self.later_id();
                    self.later_turns.insert(id);
                    self.sent = 1;
                    return Reply {
                        writes: vec![self.start_turn(id, &text)],
                        ..Reply::default()
                    };
                }
                self.finish()
            }
            _ => Reply::default(),
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
        self.pending
            .insert(key.clone(), (id, method.to_string(), params));
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
                // The step ends with what the changes add and remove
                // between them. A change's `diff` is the whole file for an
                // add or a delete, and a unified diff for an update.
                let counts: Vec<(usize, usize)> = item
                    .get("changes")
                    .and_then(Value::as_array)
                    .map(|changes| {
                        changes
                            .iter()
                            .filter_map(|c| {
                                let diff = c.get("diff")?.as_str()?;
                                Some(match c.pointer("/kind/type").and_then(Value::as_str) {
                                    Some("add") => (diff.lines().count(), 0),
                                    Some("delete") => (0, diff.lines().count()),
                                    _ => crate::parse::diff_counts(diff),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let counts = (!counts.is_empty()).then(|| {
                    let (added, removed) = counts
                        .iter()
                        .fold((0, 0), |(a, r), (da, dr)| (a + da, r + dr));
                    format!(" {}", crate::parse::counts_suffix(added, removed))
                });
                vec![
                    RunEvent::ToolCall {
                        id: id.clone(),
                        name: "file_change".into(),
                        summary: format!(
                            "{}{}",
                            one_line(&format!("Edited {}", names.join(", ")), 100),
                            counts.unwrap_or_default()
                        ),
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
            // The prompt and each steer come back as the turn reads them.
            "userMessage" => {
                let text: Vec<String> = item
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|parts| parts.iter().map(|p| str_of(p, "text")).collect())
                    .unwrap_or_default();
                if self.steers.front() == Some(&text.join("")) {
                    self.sent = self.sent.saturating_sub(1);
                    self.steers
                        .pop_front()
                        .map(RunEvent::Steered)
                        .into_iter()
                        .collect()
                } else {
                    Vec::new()
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

/// What Codex's automatic reviewer decided about a request, as a step:
/// the command and the decision, and its reason when it refused.
fn auto_review_step(params: &Value) -> Vec<RunEvent> {
    let id = format!("review-{}", str_of(params, "reviewId"));
    let status = params
        .pointer("/review/status")
        .and_then(Value::as_str)
        .unwrap_or("");
    let approved = status == "approved";
    let action = params.get("action").cloned().unwrap_or(Value::Null);
    let what = match str_of(&action, "command") {
        command if !command.is_empty() => strip_shell(&command),
        _ => str_of(&action, "type"),
    };
    let risk = params
        .pointer("/review/riskLevel")
        .and_then(Value::as_str)
        .map(|risk| format!(" (risk: {risk})"))
        .unwrap_or_default();
    let summary = if approved {
        format!("Approved automatically: {what}{risk}")
    } else {
        let why = params
            .pointer("/review/rationale")
            .and_then(Value::as_str)
            .unwrap_or("");
        format!("Refused automatically: {what}{risk}. {why}")
    };
    vec![
        RunEvent::ToolCall {
            id: id.clone(),
            name: "auto_review".into(),
            summary: one_line(summary.trim(), 200),
            parent_id: None,
        },
        RunEvent::ToolResult {
            id,
            ok: approved,
            summary: String::new(),
        },
    ]
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// What a Codex permissions request asks for, in a line.
fn permissions_summary(permissions: &Value) -> String {
    let mut parts = Vec::new();
    if permissions
        .pointer("/network/enabled")
        .and_then(Value::as_bool)
        == Some(true)
    {
        parts.push("network access".to_string());
    }
    let writes: Vec<String> = permissions
        .pointer("/fileSystem/write")
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
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

        let allowed: Value =
            serde_json::from_str(&host.answer("c1", &Decision::Allow).unwrap()).unwrap();
        assert_eq!(allowed["response"]["request_id"], "c1");
        assert_eq!(allowed["response"]["response"]["behavior"], "allow");
        assert_eq!(
            allowed["response"]["response"]["updatedInput"]["command"], "git commit -m x",
            "the input goes back as it came"
        );
        assert_eq!(host.answer("c1", &Decision::Allow), None, "answered once");
        let denied: Value = serde_json::from_str(
            &host
                .answer("n1", &Decision::deny("The reader said no"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(denied["response"]["response"]["behavior"], "deny");
        assert_eq!(
            denied["response"]["response"]["message"],
            "The reader said no"
        );
    }

    #[test]
    fn claude_other_control_requests_are_refused_and_the_result_closes_stdin() {
        let mut host = ClaudeHost::new(String::new());
        let hook = host.after_line(&line(
            r#"{"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback"}}"#,
        ));
        assert!(hook.events.is_empty());
        assert!(hook.writes[0].contains("\"subtype\":\"error\""));
        assert!(
            host.after_line(&line(r#"{"type":"result","subtype":"success"}"#))
                .close
        );
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
            approval_policy: "on-request".into(),
            reviewer: Some("user".into()),
            sandbox: None,
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
        assert_eq!(
            start["params"]["config"]["default_permissions"],
            "perch_work"
        );
        let after_thread =
            server.after_line(&line(r#"{"id":2,"result":{"thread":{"id":"th-1"}}}"#));
        let turn: Value = serde_json::from_str(&after_thread.writes[0]).unwrap();
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["threadId"], "th-1");
        assert_eq!(turn["params"]["input"][0]["text"], "Do it");
        assert_eq!(
            after_thread.events,
            vec![RunEvent::Started {
                session_id: "th-1".into(),
                model: "gpt".into()
            }]
        );

        // A resumed thread is unarchived first, and resumed whether or
        // not it was archived.
        for answer in [
            r#"{"id":4,"result":{"thread":{"id":"th-0"}}}"#,
            r#"{"id":4,"error":{"code":-32600,"message":"thread th-0 is not archived"}}"#,
        ] {
            let mut resumed = CodexServer::new(thread(Some("th-0")));
            resumed.opening();
            let unarchive: Value = serde_json::from_str(
                &resumed.after_line(&line(r#"{"id":1,"result":{}}"#)).writes[1],
            )
            .unwrap();
            assert_eq!(unarchive["method"], "thread/unarchive");
            assert_eq!(unarchive["params"]["threadId"], "th-0");
            let after = resumed.after_line(&line(answer));
            assert!(after.events.is_empty() && !after.close);
            let start: Value = serde_json::from_str(&after.writes[0]).unwrap();
            assert_eq!(start["method"], "thread/resume");
            assert_eq!(start["params"]["threadId"], "th-0");
        }
    }

    #[test]
    fn codex_items_become_steps_and_the_turn_ends_the_run() {
        let mut server = CodexServer::new(thread(None));
        server.after_line(&line(r#"{"id":2,"result":{"thread":{"id":"th-1"}}}"#));
        let started = server.after_line(&line(
            r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"c1","command":"/bin/zsh -lc ls"}}}"#,
        ));
        assert!(
            matches!(&started.events[0], RunEvent::ToolCall { summary, .. } if summary == "Ran ls")
        );
        let done = server.after_line(&line(
            r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"c1","command":"/bin/zsh -lc ls","exitCode":1,"aggregatedOutput":"no such file\n"}}}"#,
        ));
        assert_eq!(
            done.events,
            vec![RunEvent::ToolResult {
                id: "c1".into(),
                ok: false,
                summary: "no such file".into()
            }]
        );
        let edit = server.after_line(&line(
            r#"{"method":"item/completed","params":{"item":{"type":"fileChange","id":"f1","changes":[{"path":"/wt/notes.md"}],"status":"completed"}}}"#,
        ));
        assert!(
            matches!(&edit.events[0], RunEvent::ToolCall { summary, .. } if summary == "Edited notes.md")
        );
        // With diffs, the step ends with what they add and remove: all of
        // an added file, and an update's own lines.
        let counted = server.after_line(&line(
            r#"{"method":"item/completed","params":{"item":{"type":"fileChange","id":"f2","changes":[{"path":"/wt/a.md","kind":{"type":"add"},"diff":"one\ntwo\n"},{"path":"/wt/b.rs","kind":{"type":"update"},"diff":"--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-old\n+new\n"}],"status":"completed"}}}"#,
        ));
        assert!(matches!(
            &counted.events[0],
            RunEvent::ToolCall { summary, .. } if summary == "Edited a.md, b.rs (+3 \u{2212}1)"
        ));
        let said = server.after_line(&line(
            r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m1","text":"Done."}}}"#,
        ));
        assert_eq!(said.events, vec![RunEvent::Text("Done.".into())]);
        let usage = server.after_line(&line(
            r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":10,"outputTokens":2,"cachedInputTokens":1}}}}"#,
        ));
        assert_eq!(
            usage.events,
            vec![RunEvent::Usage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                cached_input_tokens: Some(1)
            }]
        );
        let end = server.after_line(&line(
            r#"{"method":"turn/completed","params":{"turn":{"status":"completed"}}}"#,
        ));
        assert!(
            matches!(&end.events[0], RunEvent::Finished { session_id, .. } if session_id == "th-1")
        );
        // The thread is archived, out of the Codex app's list, before
        // stdin closes.
        assert!(!end.close, "stdin stays open for the archive");
        let archive: Value = serde_json::from_str(&end.writes[0]).unwrap();
        assert_eq!(archive["method"], "thread/archive");
        assert_eq!(archive["params"]["threadId"], "th-1");
        let late = server.after_line(&line(
            r#"{"method":"thread/status/changed","params":{"threadId":"th-1"}}"#,
        ));
        assert_eq!(late, Reply::default());
        let archived = server.after_line(&line(r#"{"id":5,"result":{}}"#));
        assert!(archived.close && archived.events.is_empty());
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
        let accept: Value =
            serde_json::from_str(&server.answer("7", &Decision::Allow).unwrap()).unwrap();
        assert_eq!(accept["id"], 7, "the server's id, as it sent it");
        assert_eq!(accept["result"]["decision"], "accept");
        let decline: Value =
            serde_json::from_str(&server.answer("x8", &Decision::deny("no")).unwrap()).unwrap();
        assert_eq!(decline["id"], "x8");
        assert_eq!(decline["result"]["decision"], "decline");

        let other = server.after_line(&line(r#"{"method":"item/tool/call","id":9,"params":{}}"#));
        assert!(other.events.is_empty());
        assert!(other.writes[0].contains("\"error\""));
    }

    #[test]
    fn codex_s_automatic_reviewer_leaves_a_step_for_each_decision() {
        let mut server = CodexServer::new(CodexThread {
            reviewer: Some("auto_review".into()),
            ..thread(None)
        });
        server.opening();
        let start: Value =
            serde_json::from_str(&server.after_line(&line(r#"{"id":1,"result":{}}"#)).writes[1])
                .unwrap();
        assert_eq!(start["params"]["approvalsReviewer"], "auto_review");

        let approved = server.after_line(&line(
            r#"{"method":"item/autoApprovalReview/completed","params":{"reviewId":"r1","review":{"status":"approved","riskLevel":"low","rationale":"asked for"},"action":{"type":"command","command":"/bin/zsh -lc 'curl https://example.com'"}}}"#,
        ));
        assert!(
            matches!(
                &approved.events[..],
                [RunEvent::ToolCall { summary, .. }, RunEvent::ToolResult { ok: true, .. }]
                    if summary == "Approved automatically: curl https://example.com (risk: low)"
            ),
            "{:?}",
            approved.events
        );
        let refused = server.after_line(&line(
            r#"{"method":"item/autoApprovalReview/completed","params":{"reviewId":"r2","review":{"status":"denied","riskLevel":"high","rationale":"Deletes the home directory."},"action":{"type":"command","command":"rm -rf ~"}}}"#,
        ));
        assert!(
            matches!(
                &refused.events[..],
                [RunEvent::ToolCall { summary, .. }, RunEvent::ToolResult { ok: false, .. }]
                    if summary == "Refused automatically: rm -rf ~ (risk: high). Deletes the home directory."
            ),
            "{:?}",
            refused.events
        );
    }

    #[test]
    fn claude_steers_land_as_replayed_and_hold_the_result_until_they_do() {
        let mut host = ClaudeHost::new("Do it".into());
        host.opening();
        let prompt = host.after_line(&line(
            r#"{"type":"user","message":{"role":"user","content":"Do it"},"parent_tool_use_id":null,"isReplay":true}"#,
        ));
        assert!(prompt.events.is_empty(), "the prompt is not a steer");
        let wrote: Value = serde_json::from_str(&host.steer("And the docs")[0]).unwrap();
        assert_eq!(wrote["message"]["content"], "And the docs");
        let held = host.after_line(&line(r#"{"type":"result","subtype":"success"}"#));
        assert!(
            held.more && !held.close,
            "a steer not yet read keeps stdin open"
        );
        let subagent = host.after_line(&line(
            r#"{"type":"user","message":{"role":"user","content":"And the docs"},"parent_tool_use_id":"t1","isReplay":true}"#,
        ));
        assert!(
            subagent.events.is_empty(),
            "a subagent's prompt is not the reader's"
        );
        let landed = host.after_line(&line(
            r#"{"type":"user","message":{"role":"user","content":"And the docs"},"parent_tool_use_id":null,"isReplay":true}"#,
        ));
        assert_eq!(
            landed.events,
            vec![RunEvent::Steered("And the docs".into())]
        );
        let end = host.after_line(&line(r#"{"type":"result","subtype":"success"}"#));
        assert!(end.close && !end.more);
    }

    #[test]
    fn a_codex_steer_waits_for_its_turn_and_a_refused_later_turn_ends_the_run() {
        let mut server = CodexServer::new(thread(None));
        server.after_line(&line(r#"{"id":2,"result":{"thread":{"id":"th-1"}}}"#));
        assert!(
            server.steer("And the docs").is_empty(),
            "no turn to name yet"
        );
        let turn = server.after_line(&line(r#"{"id":3,"result":{"turn":{"id":"tu-1"}}}"#));
        let steer: Value = serde_json::from_str(&turn.writes[0]).unwrap();
        assert_eq!(steer["method"], "turn/steer");
        assert_eq!(steer["params"]["expectedTurnId"], "tu-1");
        let missed = server.after_line(&line(
            r#"{"id":steer_id,"error":{"message":"no active turn to steer"}}"#
                .replace("steer_id", &steer["id"].to_string())
                .as_str(),
        ));
        assert_eq!(missed, Reply::default(), "the turn's end resends it");
        let next = server.after_line(&line(
            r#"{"method":"turn/completed","params":{"turn":{"status":"completed"}}}"#,
        ));
        assert!(!next.close && next.events.is_empty());
        let start: Value = serde_json::from_str(&next.writes[0]).unwrap();
        assert_eq!(start["method"], "turn/start");
        let refused = server.after_line(&line(&format!(
            r#"{{"id":{},"error":{{"message":"busy"}}}}"#,
            start["id"]
        )));
        assert!(!refused.close && refused.writes[0].contains("thread/archive"));
        assert!(
            matches!(&refused.events[..], [RunEvent::Finished { .. }]),
            "{:?}",
            refused.events
        );
    }

    #[test]
    fn codex_failures_end_the_run() {
        let mut server = CodexServer::new(thread(None));
        let refused = server.after_line(&line(
            r#"{"id":2,"error":{"code":1,"message":"no such model"}}"#,
        ));
        assert!(refused.close);
        assert_eq!(
            refused.events,
            vec![RunEvent::Failed {
                message: "no such model".into()
            }]
        );

        let mut server = CodexServer::new(thread(None));
        let failed = server.after_line(&line(
            r#"{"method":"turn/completed","params":{"turn":{"status":"failed","error":{"message":"quota"}}}}"#,
        ));
        assert_eq!(
            failed.events,
            vec![RunEvent::Failed {
                message: "quota".into()
            }]
        );
        assert!(failed.close, "no thread to archive");

        // A failed resume archives the thread again; a refused archive
        // still ends the run.
        let mut server = CodexServer::new(thread(Some("th-0")));
        server.after_line(&line(r#"{"id":4,"result":{}}"#));
        let refused = server.after_line(&line(r#"{"id":2,"error":{"message":"gone"}}"#));
        let archive: Value = serde_json::from_str(&refused.writes[0]).unwrap();
        assert_eq!(archive["params"]["threadId"], "th-0");
        let archived = server.after_line(&line(r#"{"id":5,"error":{"message":"no rollout"}}"#));
        assert!(archived.close);
    }
}
