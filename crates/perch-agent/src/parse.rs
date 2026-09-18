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

    match (kind, subtype) {
        ("system", "init") => vec![RunEvent::Started {
            session_id: str_of(&v, "session_id"),
            model: str_of(&v, "model"),
        }],
        ("assistant", _) => blocks(&v)
            .iter()
            .filter_map(|b| match b.get("type").and_then(Value::as_str) {
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
                let message = if text.trim().is_empty() { subtype.replace('_', " ") } else { text };
                vec![RunEvent::Failed { message }]
            } else {
                vec![RunEvent::Finished {
                    session_id: str_of(&v, "session_id"),
                    cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
                    duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
                }]
            }
        }
        // thinking_tokens, task_summary, rate limits, and whatever comes next.
        _ => Vec::new(),
    }
}

fn blocks(v: &Value) -> Vec<Value> {
    v.pointer("/message/content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn str_of(v: &Value, key: &str) -> String {
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
        "Read" => format!("Read {}", field("file_path").map(|p| short_path(&p)).unwrap_or_default()),
        "Glob" => format!("Listed {}", field("pattern").unwrap_or_default()),
        "LS" => format!("Listed {}", field("path").map(|p| short_path(&p)).unwrap_or_else(|| ".".into())),
        "Grep" => {
            let pattern = field("pattern").unwrap_or_default();
            match field("path") {
                Some(p) => format!("Searched for {pattern} in {}", short_path(&p)),
                None => format!("Searched for {pattern}"),
            }
        }
        "Bash" => format!("Ran {}", strip_cd(&field("command").unwrap_or_default())),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            format!("Asked to edit {}", field("file_path").map(|p| short_path(&p)).unwrap_or_default())
        }
        other => match field("file_path").or_else(|| field("path")).or_else(|| field("pattern")) {
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

fn one_line(text: &str, max: usize) -> String {
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
        assert_eq!(summarize_tool_call("Grep", &json!({"pattern": "epoch"})), "Searched for epoch");
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
            summarize_tool_call("Bash", &json!({"command": "cd \"/a b/wt\" && git log -3 -- src/"})),
            "Ran git log -3 -- src/"
        );
        assert_eq!(summarize_tool_call("Bash", &json!({"command": "cd src"})), "Ran cd src");
    }

    #[test]
    fn an_unknown_tool_still_gets_a_line() {
        assert_eq!(summarize_tool_call("WebFetch", &json!({"url": "https://x"})), "WebFetch");
        assert_eq!(summarize_tool_call("Thing", &json!({"path": "a/b"})), "Thing a/b");
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
    fn an_error_result_is_a_failure() {
        let events = parse_line(
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"","session_id":"s"}"#,
        );
        assert_eq!(events, vec![RunEvent::Failed { message: "error max turns".into() }]);
    }
}
