//! From a line of the agent's stdout to events, and from a tool call to
//! the one line the UI shows for it. Both pure, both pinned by fixtures.

use serde_json::Value;

use crate::RunEvent;

/// Claude Code's tool for filing review findings as data rather than prose.
pub const REPORT_TOOL: &str = "ReportFindings";

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
    let parent_id = v
        .get("parent_tool_use_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let delegated = parent_id.is_some();

    match (kind, subtype) {
        ("system", "init") => vec![RunEvent::Started {
            session_id: str_of(&v, "session_id"),
            model: str_of(&v, "model"),
        }],
        ("assistant", _) => blocks(&v)
            .iter()
            .flat_map(|b| match b.get("type").and_then(Value::as_str) {
                Some("text") if delegated => Vec::new(),
                Some("text") => {
                    let text = str_of(b, "text");
                    (!text.trim().is_empty())
                        .then_some(RunEvent::Text(text))
                        .into_iter()
                        .collect()
                }
                Some("tool_use") => {
                    let name = str_of(b, "name");
                    let input = b.get("input").cloned().unwrap_or(Value::Null);
                    let mut events = vec![RunEvent::ToolCall {
                        id: str_of(b, "id"),
                        summary: summarize_tool_call(&name, &input),
                        name: name.clone(),
                        parent_id: parent_id.clone(),
                    }];
                    // The call is the report: its input is the findings,
                    // and the prose that follows is only a summary.
                    if name == REPORT_TOOL {
                        events.push(RunEvent::Report {
                            tool: name,
                            input,
                            parent_id: parent_id.clone(),
                        });
                    }
                    events
                }
                _ => Vec::new(),
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
        ("system", "permission_denied") => vec![permission_denied_event(&v)],
        ("result", _) => {
            let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            if is_error || (subtype != "success" && !subtype.is_empty()) {
                let text = str_of(&v, "result");
                let message = if text.trim().is_empty() {
                    subtype.replace('_', " ")
                } else {
                    text
                };
                let mut events = Vec::new();
                if v.get("permission_denials")
                    .and_then(Value::as_array)
                    .is_some_and(|denials| !denials.is_empty())
                {
                    events.push(permission_denied_event(&v));
                }
                events.push(RunEvent::Failed { message });
                events
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
        // thinking_tokens, task_summary, task_started and its progress, rate
        // limits (the board in `usage` takes those), and whatever comes next.
        _ => Vec::new(),
    }
}

fn permission_denied_event(v: &Value) -> RunEvent {
    let denial = v
        .get("permission_denials")
        .and_then(Value::as_array)
        .and_then(|denials| denials.first());
    let tool = v
        .get("tool_name")
        .and_then(Value::as_str)
        .or_else(|| {
            denial
                .and_then(|entry| entry.get("tool_name"))
                .and_then(Value::as_str)
        })
        .map(str::to_string);
    let message = v
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| {
            denial
                .and_then(|entry| entry.get("message"))
                .and_then(Value::as_str)
        })
        .filter(|message| !message.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match tool.as_deref() {
            Some(tool) => format!("Permission denied for {tool}"),
            None => "Permission denied".to_string(),
        });
    RunEvent::PermissionDenied { tool, message }
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
        // A command let out of the sandbox says so: in a Work run an
        // automatic reviewer may have let it, and the step is how the
        // reader sees that.
        "Bash"
            if input
                .get("dangerouslyDisableSandbox")
                .and_then(Value::as_bool)
                == Some(true) =>
        {
            format!(
                "Ran outside the sandbox: {}",
                strip_cd(&field("command").unwrap_or_default())
            )
        }
        // In auto mode a command can name the hosts it needs, which the
        // classifier reviews with it; the step says which.
        "Bash" => {
            let command = strip_cd(&field("command").unwrap_or_default());
            let hosts: Vec<&str> = input
                .get("allowed_domains")
                .and_then(Value::as_array)
                .map(|hosts| hosts.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if hosts.is_empty() {
                format!("Ran {command}")
            } else {
                format!("Ran {command} (network: {})", hosts.join(", "))
            }
        }
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let path = field("file_path")
                .map(|p| short_path(&p))
                .unwrap_or_default();
            match edit_counts(name, input) {
                Some((added, removed)) => {
                    format!("Asked to edit {path} {}", counts_suffix(added, removed))
                }
                None => format!("Asked to edit {path}"),
            }
        }
        "Agent" | "Task" => match field("description") {
            Some(d) => format!("Delegated: {d}"),
            None => "Delegated".to_string(),
        },
        // The input is a whole script; the line says only that one ran.
        "Workflow" => "Ran a workflow".to_string(),
        REPORT_TOOL => match input
            .get("findings")
            .and_then(Value::as_array)
            .map(Vec::len)
        {
            Some(0) => "Reported no findings".to_string(),
            Some(1) => "Reported 1 finding".to_string(),
            Some(count) => format!("Reported {count} findings"),
            None => "Reported findings".to_string(),
        },
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

/// The lines an edit adds and removes, from what it replaces and what it
/// replaces it with: the lines both share at the start and the end are
/// left out, so a one-line change in a long `old_string` counts as one.
/// A whole file written counts every line as added.
fn edit_counts(name: &str, input: &Value) -> Option<(usize, usize)> {
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    match name {
        "Edit" => Some(changed_lines(
            &text(input, "old_string")?,
            &text(input, "new_string")?,
        )),
        "MultiEdit" => {
            let edits = input.get("edits")?.as_array()?;
            Some(edits.iter().fold((0, 0), |(added, removed), edit| {
                let (a, r) = changed_lines(
                    &text(edit, "old_string").unwrap_or_default(),
                    &text(edit, "new_string").unwrap_or_default(),
                );
                (added + a, removed + r)
            }))
        }
        "Write" => Some((text(input, "content")?.lines().count(), 0)),
        _ => None,
    }
}

fn changed_lines(old: &str, new: &str) -> (usize, usize) {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let head = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let tail = old[head..]
        .iter()
        .rev()
        .zip(new[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (new.len() - head - tail, old.len() - head - tail)
}

/// The lines a unified diff adds and removes, less its file headers.
pub(crate) fn diff_counts(diff: &str) -> (usize, usize) {
    diff.lines().fold((0, 0), |(added, removed), line| {
        if line.starts_with('+') && !line.starts_with("+++") {
            (added + 1, removed)
        } else if line.starts_with('-') && !line.starts_with("---") {
            (added, removed + 1)
        } else {
            (added, removed)
        }
    })
}

/// How an edit step ends, `(+12 −3)`, which the thread reads back to show
/// the counts in their colours.
pub fn counts_suffix(added: usize, removed: usize) -> String {
    format!("(+{added} \u{2212}{removed})")
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
pub(crate) fn short_path(path: &str) -> String {
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

    #[test]
    fn an_edit_says_the_lines_it_adds_and_removes() {
        let edit = serde_json::json!({
            "file_path": "/wt/src/db.rs",
            "old_string": "fn a() {\n    1\n}",
            "new_string": "fn a() {\n    2\n    3\n}",
        });
        assert_eq!(
            summarize_tool_call("Edit", &edit),
            "Asked to edit wt/src/db.rs (+2 \u{2212}1)"
        );
        let many = serde_json::json!({
            "file_path": "a.rs",
            "edits": [{"old_string": "x", "new_string": "y"}, {"old_string": "", "new_string": "z"}],
        });
        assert_eq!(
            summarize_tool_call("MultiEdit", &many),
            "Asked to edit a.rs (+2 \u{2212}1)"
        );
        let write = serde_json::json!({"file_path": "a.rs", "content": "one\ntwo\n"});
        assert_eq!(
            summarize_tool_call("Write", &write),
            "Asked to edit a.rs (+2 \u{2212}0)"
        );
        let notebook = serde_json::json!({"file_path": "n.ipynb"});
        assert_eq!(
            summarize_tool_call("NotebookEdit", &notebook),
            "Asked to edit n.ipynb"
        );
    }

    #[test]
    fn a_command_let_out_or_given_hosts_says_so() {
        let out = serde_json::json!({"command": "echo x > ~/f", "dangerouslyDisableSandbox": true});
        assert_eq!(
            summarize_tool_call("Bash", &out),
            "Ran outside the sandbox: echo x > ~/f"
        );
        let hosts = serde_json::json!({"command": "curl https://example.com", "allowed_domains": ["example.com"]});
        assert_eq!(
            summarize_tool_call("Bash", &hosts),
            "Ran curl https://example.com (network: example.com)"
        );
    }
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
                summary: "Read w/src/lib.rs".into(),
                parent_id: Some(parent.into()),
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
    fn a_filed_report_arrives_as_its_call_and_as_the_report() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"ReportFindings","input":{"findings":[{"file":"src/lib.rs","line":4,"summary":"s","failure_scenario":"f"}],"level":"low"}}]},"parent_tool_use_id":null,"session_id":"s"}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            RunEvent::ToolCall {
                id: "toolu_1".into(),
                name: "ReportFindings".into(),
                summary: "Reported 1 finding".into(),
                parent_id: None,
            }
        );
        let RunEvent::Report {
            tool,
            input,
            parent_id,
        } = &events[1]
        else {
            panic!("no report: {events:?}");
        };
        assert_eq!(tool, "ReportFindings");
        assert_eq!(input["findings"][0]["line"], 4);
        assert!(parent_id.is_none());

        let empty = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_2","name":"ReportFindings","input":{"findings":[],"level":"low"}}]},"parent_tool_use_id":"toolu_parent","session_id":"s"}"#;
        let events = parse_line(empty);
        assert!(
            matches!(&events[0], RunEvent::ToolCall { summary, parent_id: Some(_), .. } if summary == "Reported no findings")
        );
        assert!(
            matches!(&events[1], RunEvent::Report { parent_id: Some(parent), .. } if parent == "toolu_parent")
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

    #[test]
    fn structured_permission_denials_are_visible_activity() {
        assert_eq!(
            parse_line(
                r#"{"type":"system","subtype":"permission_denied","tool_name":"Write","message":"Write was denied"}"#
            ),
            vec![RunEvent::PermissionDenied {
                tool: Some("Write".into()),
                message: "Write was denied".into(),
            }]
        );

        assert_eq!(
            parse_line(
                r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"review stopped","permission_denials":[{"tool_name":"Edit"}]}"#
            ),
            vec![
                RunEvent::PermissionDenied {
                    tool: Some("Edit".into()),
                    message: "Permission denied for Edit".into(),
                },
                RunEvent::Failed {
                    message: "review stopped".into(),
                },
            ]
        );
    }
}
