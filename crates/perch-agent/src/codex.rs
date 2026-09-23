//! From a line of `codex exec --json` to events. Codex names its thread
//! once, at the start, and reports a command twice, when it begins and
//! when it ends, so unlike the Claude Code parser this one keeps state
//! between lines: the thread id for `Finished`, and the ids of commands
//! that have begun so a completion never marks the wrong step.

use std::collections::HashSet;

use serde_json::Value;

use crate::parse::{one_line, str_of};
use crate::RunEvent;

#[derive(Debug, Default)]
pub struct CodexParser {
    thread_id: String,
    started: HashSet<String>,
    /// Top-level errors carry no id, so they are numbered as they come.
    errors: usize,
}

impl CodexParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// The events on one line. Usually one; a completed command that was
    /// never seen to start yields the pair that makes it a step anyway.
    pub fn parse_line(&mut self, line: &str) -> Vec<RunEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "thread.started" => {
                self.thread_id = str_of(&v, "thread_id");
                vec![RunEvent::Started {
                    session_id: self.thread_id.clone(),
                    model: String::new(),
                }]
            }
            "item.started" => self.item_started(v.get("item").unwrap_or(&Value::Null)),
            "item.completed" => self.item_completed(v.get("item").unwrap_or(&Value::Null)),
            "error" => {
                self.errors += 1;
                failed_step(
                    format!("error_{}", self.errors),
                    &human_message(&str_of(&v, "message")),
                )
            }
            "turn.failed" => {
                let raw = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let message = human_message(raw);
                // The thread prints the message as the error line, so an
                // empty one would fail the run in silence.
                let message = if message.is_empty() {
                    "The turn failed".to_string()
                } else {
                    message
                };
                vec![RunEvent::Failed { message }]
            }
            "turn.completed" => {
                let usage = v.get("usage").unwrap_or(&Value::Null);
                let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
                let output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
                let cached_input_tokens = usage.get("cached_input_tokens").and_then(Value::as_u64);
                let mut events = Vec::with_capacity(2);
                if input_tokens.is_some()
                    || output_tokens.is_some()
                    || cached_input_tokens.is_some()
                {
                    events.push(RunEvent::Usage {
                        input_tokens,
                        output_tokens,
                        cached_input_tokens,
                    });
                }
                events.push(RunEvent::Finished {
                    session_id: self.thread_id.clone(),
                    cost_usd: None,
                    duration_ms: 0,
                });
                events
            }
            // turn.started, item.updated, and whatever comes next.
            _ => Vec::new(),
        }
    }

    fn item_started(&mut self, item: &Value) -> Vec<RunEvent> {
        if str_of(item, "type") != "command_execution" {
            return Vec::new();
        }
        let id = str_of(item, "id");
        self.started.insert(id.clone());
        vec![command_call(id, item)]
    }

    fn item_completed(&mut self, item: &Value) -> Vec<RunEvent> {
        let id = str_of(item, "id");
        match str_of(item, "type").as_str() {
            "command_execution" => {
                let ok = item.get("exit_code").and_then(Value::as_i64) == Some(0);
                let summary = if ok {
                    String::new()
                } else {
                    last_line(&str_of(item, "aggregated_output"))
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
            "agent_message" => {
                let text = str_of(item, "text");
                if text.trim().is_empty() {
                    Vec::new()
                } else {
                    vec![RunEvent::Text(text)]
                }
            }
            "error" => failed_step(id, &human_message(&str_of(item, "message"))),
            // reasoning, file_change, mcp_tool_call, web_search, todo_list.
            _ => Vec::new(),
        }
    }
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

/// An error is shown as a step that failed, not as the end of the run: a
/// notice like "recorded with another model" precedes a turn that goes on
/// to complete, and an API error precedes the `turn.failed` that ends it.
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

/// The sentence in an error. Codex passes API errors through as their
/// JSON body, and the reader wants the message inside it, not the body.
pub(crate) fn human_message(raw: &str) -> String {
    let inner = serde_json::from_str::<Value>(raw.trim())
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let text = inner.unwrap_or_else(|| raw.to_string());
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn last_line(output: &str) -> String {
    let line = output
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    one_line(line, 100)
}

/// Codex runs every command through a login shell, `/bin/zsh -lc "..."`,
/// and the reader wants the command, not the shell.
pub(crate) fn strip_shell(command: &str) -> String {
    let trimmed = command.trim();
    let mut parts = trimmed.splitn(3, ' ');
    let (Some(shell), Some(flag), Some(script)) = (parts.next(), parts.next(), parts.next()) else {
        return trimmed.to_string();
    };
    let shell = shell.rsplit('/').next().unwrap_or(shell);
    let is_shell =
        matches!(shell, "zsh" | "bash" | "sh") && flag.starts_with('-') && flag.contains('c');
    if !is_shell {
        return trimmed.to_string();
    }
    unquote(script.trim())
}

/// Strip one layer of shell quoting. Inside double quotes a backslash
/// escapes the next character; inside single quotes nothing does.
fn unquote(script: &str) -> String {
    let inner = |q: char| {
        (script.len() >= 2 && script.starts_with(q) && script.ends_with(q))
            .then(|| &script[1..script.len() - 1])
    };
    if let Some(inner) = inner('\'') {
        return inner.to_string();
    }
    let Some(inner) = inner('"') else {
        return script.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        // The escaped character goes in as itself, and is consumed here
        // so an escaped backslash cannot start a second escape.
        if c == '\\' && matches!(chars.peek(), Some('"' | '\\' | '$' | '`')) {
            out.extend(chars.next());
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str =
        r#"{"type":"thread.started","thread_id":"01a0b584-d485-7d52-84b0-231e5b9b82d4"}"#;
    const CMD_STARTED: &str = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc \"sed -n '1,240p' main.rs && git log --oneline\"","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
    const CMD_COMPLETED: &str = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc \"sed -n '1,240p' main.rs && git log --oneline\"","aggregated_output":"fn main() {}\n\nabc123 init\n","exit_code":0,"status":"completed"}}"#;
    const CMD_FAILED: &str = r#"{"type":"item.completed","item":{"id":"item_3","type":"command_execution","command":"/bin/zsh -lc 'cat nope.txt'","aggregated_output":"cat: nope.txt: No such file or directory\n","exit_code":1,"status":"completed"}}"#;
    const MESSAGE: &str = r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"`main.rs` defines a Rust program that prints `hi`."}}"#;
    const ITEM_ERROR: &str = r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"This session was recorded with model `gpt-6-astra` but is resuming with `gpt-5.6-luna`. Consider switching back."}}"#;
    const TOP_ERROR: &str = r#"{"type":"error","message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'gpt-6-astra' model requires a newer version of Codex.\"}}"}"#;
    const TURN_FAILED: &str = r#"{"type":"turn.failed","error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'gpt-6-astra' model requires a newer version of Codex.\"}}"}}"#;
    const TURN_COMPLETED: &str = r#"{"type":"turn.completed","usage":{"input_tokens":40389,"cached_input_tokens":25088,"cache_write_input_tokens":0,"output_tokens":214,"reasoning_output_tokens":41}}"#;

    fn parse_all(lines: &[&str]) -> Vec<RunEvent> {
        let mut p = CodexParser::new();
        lines.iter().flat_map(|l| p.parse_line(l)).collect()
    }

    #[test]
    fn the_thread_starts_the_run_and_ends_it() {
        let events = parse_all(&[THREAD, r#"{"type":"turn.started"}"#, TURN_COMPLETED]);
        assert_eq!(
            events,
            vec![
                RunEvent::Started {
                    session_id: "01a0b584-d485-7d52-84b0-231e5b9b82d4".into(),
                    model: "".into()
                },
                RunEvent::Usage {
                    input_tokens: Some(40389),
                    output_tokens: Some(214),
                    cached_input_tokens: Some(25088),
                },
                RunEvent::Finished {
                    session_id: "01a0b584-d485-7d52-84b0-231e5b9b82d4".into(),
                    cost_usd: None,
                    duration_ms: 0,
                },
            ]
        );
    }

    #[test]
    fn a_command_is_a_call_when_it_starts_and_a_result_when_it_ends() {
        let events = parse_all(&[CMD_STARTED, CMD_COMPLETED]);
        assert_eq!(
            events,
            vec![
                RunEvent::ToolCall {
                    id: "item_1".into(),
                    name: "command_execution".into(),
                    summary: "Ran sed -n '1,240p' main.rs && git log --oneline".into(),
                    parent_id: None,
                },
                RunEvent::ToolResult {
                    id: "item_1".into(),
                    ok: true,
                    summary: "".into()
                },
            ]
        );
    }

    #[test]
    fn a_completed_command_never_seen_to_start_still_becomes_a_step() {
        let events = parse_all(&[CMD_FAILED]);
        assert_eq!(
            events,
            vec![
                RunEvent::ToolCall {
                    id: "item_3".into(),
                    name: "command_execution".into(),
                    summary: "Ran cat nope.txt".into(),
                    parent_id: None,
                },
                RunEvent::ToolResult {
                    id: "item_3".into(),
                    ok: false,
                    summary: "cat: nope.txt: No such file or directory".into(),
                },
            ]
        );
    }

    #[test]
    fn a_message_is_text_and_a_blank_one_is_nothing() {
        assert_eq!(
            parse_all(&[MESSAGE]),
            vec![RunEvent::Text(
                "`main.rs` defines a Rust program that prints `hi`.".into()
            )]
        );
        let blank = r#"{"type":"item.completed","item":{"id":"item_9","type":"agent_message","text":"  \n"}}"#;
        assert!(parse_all(&[blank]).is_empty());
    }

    #[test]
    fn an_error_item_is_a_failed_step_and_the_run_goes_on() {
        let events = parse_all(&[THREAD, ITEM_ERROR, MESSAGE, TURN_COMPLETED]);
        assert_eq!(
            events[1],
            RunEvent::ToolCall {
                id: "item_0".into(),
                name: "error".into(),
                summary: "This session was recorded with model `gpt-6-astra` but is resuming with `gpt-5.6-luna`. Consider switching back.".into(),
                parent_id: None,
            }
        );
        assert_eq!(
            events[2],
            RunEvent::ToolResult {
                id: "item_0".into(),
                ok: false,
                summary: "".into()
            }
        );
        assert!(matches!(events[3], RunEvent::Text(_)));
        assert!(matches!(events[4], RunEvent::Usage { .. }));
        assert!(matches!(events[5], RunEvent::Finished { .. }));
        assert!(!events.iter().any(|e| matches!(e, RunEvent::Failed { .. })));
    }

    #[test]
    fn a_top_level_error_is_a_failed_step_with_the_sentence_inside() {
        let events = parse_all(&[TOP_ERROR]);
        assert_eq!(
            events,
            vec![
                RunEvent::ToolCall {
                    id: "error_1".into(),
                    name: "error".into(),
                    summary: "The 'gpt-6-astra' model requires a newer version of Codex.".into(),
                    parent_id: None,
                },
                RunEvent::ToolResult {
                    id: "error_1".into(),
                    ok: false,
                    summary: "".into()
                },
            ]
        );
    }

    #[test]
    fn a_failed_turn_fails_the_run_with_the_sentence_inside_the_json() {
        let events = parse_all(&[THREAD, TOP_ERROR, TURN_FAILED]);
        assert_eq!(
            events.last(),
            Some(&RunEvent::Failed {
                message: "The 'gpt-6-astra' model requires a newer version of Codex.".into()
            })
        );
        let plain = r#"{"type":"turn.failed","error":{"message":"  stream closed\n"}}"#;
        assert_eq!(
            parse_all(&[plain]),
            vec![RunEvent::Failed {
                message: "stream closed".into()
            }]
        );
    }

    #[test]
    fn a_failed_turn_with_no_message_still_says_something() {
        let expected = vec![RunEvent::Failed {
            message: "The turn failed".into(),
        }];
        assert_eq!(parse_all(&[r#"{"type":"turn.failed"}"#]), expected);
        assert_eq!(
            parse_all(&[r#"{"type":"turn.failed","error":{"message":"  \n"}}"#]),
            expected
        );
    }

    #[test]
    fn other_items_and_garbage_yield_nothing() {
        assert!(parse_all(&["", "not json", r#"{"type":"turn.started"}"#]).is_empty());
        let reasoning =
            r#"{"type":"item.completed","item":{"id":"item_4","type":"reasoning","text":"hmm"}}"#;
        let started_reasoning =
            r#"{"type":"item.started","item":{"id":"item_4","type":"reasoning"}}"#;
        assert!(parse_all(&[started_reasoning, reasoning]).is_empty());
    }

    #[test]
    fn the_shell_wrapper_is_stripped_from_commands() {
        assert_eq!(
            strip_shell(r#"/bin/zsh -lc "sed -n '1,240p' main.rs && git log --oneline""#),
            "sed -n '1,240p' main.rs && git log --oneline"
        );
        assert_eq!(
            strip_shell("/bin/zsh -lc 'find crates -maxdepth 3 -type f | sort'"),
            "find crates -maxdepth 3 -type f | sort"
        );
        assert_eq!(
            strip_shell(r#"bash -lc "rg -n \"x y\" src""#),
            r#"rg -n "x y" src"#
        );
        // An escaped backslash is one backslash, and does not swallow the
        // quote after it.
        assert_eq!(
            strip_shell(r#"bash -lc "rg -n \"\\\\bfoo\" src""#),
            r#"rg -n "\\bfoo" src"#
        );
        assert_eq!(strip_shell(r#"bash -lc "echo a\\\"b""#), r#"echo a\"b"#);
        assert_eq!(strip_shell("/bin/sh -c ls"), "ls");
        assert_eq!(strip_shell("git log --oneline"), "git log --oneline");
        assert_eq!(strip_shell("python -c 'print(1)'"), "python -c 'print(1)'");
    }

    #[test]
    fn a_failing_command_reports_its_last_line_bounded() {
        let long = "e".repeat(300);
        let line = format!(
            r#"{{"type":"item.completed","item":{{"id":"item_5","type":"command_execution","command":"/bin/zsh -lc 'x'","aggregated_output":"first\n{long}\n\n","exit_code":2,"status":"completed"}}}}"#
        );
        let events = parse_all(&[&line]);
        match &events[1] {
            RunEvent::ToolResult { ok, summary, .. } => {
                assert!(!ok);
                assert!(summary.starts_with("eee"));
                assert!(summary.chars().count() <= 100);
                assert!(summary.ends_with('…'));
            }
            other => panic!("{other:?}"),
        }
    }
}
