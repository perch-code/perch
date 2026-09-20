//! From a line of the agent's stdout to events, and from a tool call to
//! the one line the UI shows for it. Both pure, both pinned by fixtures.

use serde_json::Value;

use crate::RunEvent;

/// The events on one line of `--output-format stream-json`. Usually one,
/// sometimes none, and a message with several content blocks yields one
/// per block.
pub fn parse_line(line: &str) -> Vec<RunEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("");
    // A subagent's lines name the parent's tool call. What it says is its
    // report to the parent, not part of the answer; what it does still is.
    let delegated = v
        .get("parent_tool_use_id")
        .and_then(Value::as_str)
        .is_some();

    match (kind, subtype) {
        ("system", "init") => vec![RunEvent::Started {
            session_id: str_of(&v, "session_id"),
            model: str_of(&v, "model"),
        }],
        ("assistant", _) => blocks(&v)
            .iter()
            .filter_map(|b| match b.get("type").and_then(Value::as_str) {
                Some("text") if delegated => None,
                Some("text") => {
                    let text = str_of(b, "text");
                    (!text.trim().is_empty()).then_some(RunEvent::Text(text))
                }
                Some("tool_use") => {
                    let name = str_of(b, "name");
                    let input = b.get("input").cloned().unwrap_or(Value::Null);
                    Some(RunEvent::ToolCall {
                        id: str_of(b, "id"),
                        summary: summarize_tool_call(&name, &input),
                        name,
                    })
                }
                _ => None,
            })
            .collect(),
        ("user", _) => blocks(&v)
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            .map(|b| {
                let error = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                let content = result_text(b.get("content"));
                RunEvent::ToolResult {
                    id: str_of(b, "tool_use_id"),
                    ok: !error,
                    summary: summarize_result(error, &content),
                }
            })
            .collect(),
        ("result", _) => {
            let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            if is_error || (subtype != "success" && !subtype.is_empty()) {
                let text = str_of(&v, "result");
                let message = if text.trim().is_empty() {
                    subtype.replace('_', " ")
                } else {
                    text
                };
                vec![RunEvent::Failed { message }]
            } else {
                let mut events = usage_event(&v).into_iter().collect::<Vec<_>>();
                events.push(RunEvent::Finished {
                    session_id: str_of(&v, "session_id"),
                    cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
                    duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
                });
                events
            }
        }
        // thinking_tokens, task_summary, task_started and its progress,
        // permission_denied, rate limits, and whatever comes next.
        _ => Vec::new(),
    }
}

/// Claude reports denied tools as a structured system event. It is not
/// terminal by itself: an agent can recover and finish after a denied tool.
/// The runner remembers it only so a later failed result can be classified
/// without inspecting human-readable text.
pub(crate) fn is_explicit_permission_denial(line: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    let system_event = v.get("type").and_then(Value::as_str) == Some("system")
        && v.get("subtype").and_then(Value::as_str) == Some("permission_denied");
    let result_denials = v.get("type").and_then(Value::as_str) == Some("result")
        && v.get("permission_denials")
            .and_then(Value::as_array)
            .is_some_and(|denials| !denials.is_empty());
    system_event || result_denials
}

fn usage_event(v: &Value) -> Option<RunEvent> {
    let usage = v.get("usage")?;
    let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
    let output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
    // Claude calls cache hits `cache_read_input_tokens`; accept the neutral
    // spelling as well so the event remains stable across CLI versions.
    let cached_input_tokens = usage
        .get("cached_input_tokens")
        .or_else(|| usage.get("cache_read_input_tokens"))
        .and_then(Value::as_u64);
    (input_tokens.is_some() || output_tokens.is_some() || cached_input_tokens.is_some()).then_some(
        RunEvent::Usage {
            input_tokens,
            output_tokens,
            cached_input_tokens,
        },
    )
}

fn blocks(v: &Value) -> Vec<Value> {
    v.pointer("/message/content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// A tool result's content is a string or a list of text blocks.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn summarize_result(error: bool, content: &str) -> String {
    if error && content.starts_with("Claude requested permissions") {
        return "not allowed".to_string();
    }
    if !error {
        return String::new();
    }
    one_line(content, 100)
}

/// The one line the thread shows for a tool call: "Read src/x.rs",
/// "Ran git log -5", "Searched for foo".
pub fn summarize_tool_call(name: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).map(|s| s.to_string());
    let summary = match name {
        "Read" => format!(
            "Read {}",
            field("file_path")
                .map(|p| short_path(&p))
                .unwrap_or_default()
        ),
        "Glob" => format!("Listed {}", field("pattern").unwrap_or_default()),
        "LS" => format!(
            "Listed {}",
            field("path")
                .map(|p| short_path(&p))
                .unwrap_or_else(|| ".".into())
        ),
        "Grep" => {
            let pattern = field("pattern").unwrap_or_default();
            match field("path") {
                Some(p) => format!("Searched for {pattern} in {}", short_path(&p)),
                None => format!("Searched for {pattern}"),
            }
        }
        "Bash" => format!("Ran {}", strip_cd(&field("command").unwrap_or_default())),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            format!(
                "Asked to edit {}",
                field("file_path")
                    .map(|p| short_path(&p))
                    .unwrap_or_default()
            )
        }
        "Agent" | "Task" => match field("description") {
            Some(d) => format!("Delegated: {d}"),
            None => "Delegated".to_string(),
        },
        // The input is a whole script; the line says only that one ran.
        "Workflow" => "Ran a workflow".to_string(),
        other => match field("file_path")
            .or_else(|| field("path"))
            .or_else(|| field("pattern"))
        {
            Some(arg) => format!("{other} {}", short_path(&arg)),
            None => other.to_string(),
        },
    };
    one_line(&summary, 120)
}

/// Drop a leading `cd <dir> &&`: the agent prefixes one to every command
/// when its working directory has a space in it, and the reader wants the
/// command, not the directory they are already in.
fn strip_cd(command: &str) -> String {
    let trimmed = command.trim();
    let Some(rest) = trimmed.strip_prefix("cd ") else {
        return trimmed.to_string();
    };
    // The directory may be quoted and may contain spaces; the `&&` ends it.
    match rest.split_once("&&") {
        Some((_, after)) => after.trim().to_string(),
        None => trimmed.to_string(),
    }
}

/// The last three path components, which is what a reader recognises.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 3 {
        return path.trim_start_matches('/').to_string();
    }
    parts[parts.len() - 3..].join("/")
}

pub(crate) fn one_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let cut: String = line.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_names_the_file() {
        assert_eq!(
            summarize_tool_call("Read", &json!({"file_path": "/Users/x/wt/src/manifest.rs"})),
            "Read wt/src/manifest.rs"
        );
    }

    #[test]
    fn grep_names_the_pattern_and_bash_the_command() {
        assert_eq!(
            summarize_tool_call("Grep", &json!({"pattern": "epoch"})),
            "Searched for epoch"
        );
        assert_eq!(
            summarize_tool_call("Grep", &json!({"pattern": "epoch", "path": "src"})),
            "Searched for epoch in src"
        );
        assert_eq!(
            summarize_tool_call("Bash", &json!({"command": "git log -5 -- src/"})),
            "Ran git log -5 -- src/"
        );
    }

    #[test]
    fn a_leading_cd_is_dropped_from_a_command() {
        assert_eq!(
            summarize_tool_call(
                "Bash",
                &json!({"command": "cd \"/a b/wt\" && git log -3 -- src/"})
            ),
            "Ran git log -3 -- src/"
        );
        assert_eq!(
            summarize_tool_call("Bash", &json!({"command": "cd src"})),
            "Ran cd src"
        );
    }

    #[test]
    fn delegation_names_the_task_and_a_workflow_hides_its_script() {
        assert_eq!(
            summarize_tool_call(
                "Agent",
                &json!({"description": "Read first line of lib.rs", "prompt": "..."})
            ),
            "Delegated: Read first line of lib.rs"
        );
        assert_eq!(
            summarize_tool_call("Task", &json!({"prompt": "..."})),
            "Delegated"
        );
        let s = summarize_tool_call(
            "Workflow",
            &json!({"script": "const x = 1;\nawait run(x);"}),
        );
        assert_eq!(s, "Ran a workflow");
    }

    #[test]
    fn an_unknown_tool_still_gets_a_line() {
        assert_eq!(
            summarize_tool_call("WebFetch", &json!({"url": "https://x"})),
            "WebFetch"
        );
        assert_eq!(
            summarize_tool_call("Thing", &json!({"path": "a/b"})),
            "Thing a/b"
        );
    }

    #[test]
    fn summaries_are_one_line_and_bounded() {
        let long = "x".repeat(300);
        let s = summarize_tool_call("Bash", &json!({"command": format!("{long}\nsecond line")}));
        assert!(!s.contains('\n'));
        assert!(s.chars().count() <= 120);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn garbage_lines_yield_nothing() {
        assert!(parse_line("not json").is_empty());
        assert!(parse_line("").is_empty());
        assert!(parse_line(r#"{"type":"rate_limit_event"}"#).is_empty());
    }

    #[test]
    fn a_subagent_says_nothing_but_its_steps_still_show() {
        let parent = "toolu_01WXCo8udLVT4woPKD2gWmjc";
        let text = format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"the first line is x"}}]}},"parent_tool_use_id":"{parent}","session_id":"s"}}"#
        );
        assert!(parse_line(&text).is_empty());

        let call = format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_r","name":"Read","input":{{"file_path":"/w/src/lib.rs","limit":1}}}}]}},"parent_tool_use_id":"{parent}","session_id":"s"}}"#
        );
        assert_eq!(
            parse_line(&call),
            vec![RunEvent::ToolCall {
                id: "toolu_r".into(),
                name: "Read".into(),
                summary: "Read w/src/lib.rs".into()
            }]
        );
        let result = format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"tool_use_id":"toolu_r","type":"tool_result","content":"1\t//! x"}}]}},"parent_tool_use_id":"{parent}","session_id":"s"}}"#
        );
        assert_eq!(
            parse_line(&result),
            vec![RunEvent::ToolResult {
                id: "toolu_r".into(),
                ok: true,
                summary: "".into()
            }]
        );

        // The parent's own text, with the field present but null, is the answer.
        let top = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"//! x"}]},"parent_tool_use_id":null,"session_id":"s"}"#;
        assert_eq!(parse_line(top), vec![RunEvent::Text("//! x".into())]);
    }

    #[test]
    fn an_error_result_is_a_failure() {
        let events = parse_line(
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"","session_id":"s"}"#,
        );
        assert_eq!(
            events,
            vec![RunEvent::Failed {
                message: "error max turns".into()
            }]
        );
    }

    #[test]
    fn a_success_result_reports_usage_immediately_before_finished() {
        let events = parse_line(
            r#"{"type":"result","subtype":"success","session_id":"s","usage":{"input_tokens":34,"cache_read_input_tokens":152655,"output_tokens":1163}}"#,
        );
        assert_eq!(
            events,
            vec![
                RunEvent::Usage {
                    input_tokens: Some(34),
                    output_tokens: Some(1163),
                    cached_input_tokens: Some(152655),
                },
                RunEvent::Finished {
                    session_id: "s".into(),
                    cost_usd: None,
                    duration_ms: 0
                },
            ]
        );
    }

    #[test]
    fn only_the_structured_system_event_is_a_permission_denial() {
        assert!(is_explicit_permission_denial(
            r#"{"type":"system","subtype":"permission_denied","message":"anything"}"#
        ));
        assert!(is_explicit_permission_denial(
            r#"{"type":"result","subtype":"error_during_execution","permission_denials":[{"tool_name":"Write"}]}"#
        ));
        assert!(!is_explicit_permission_denial(
            r#"{"type":"result","subtype":"error_during_execution","result":"permission denied"}"#
        ));
        assert!(!is_explicit_permission_denial("permission denied"));
    }
}
