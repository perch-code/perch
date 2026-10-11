//! The parser against lines captured from the real binary.

use perch_agent::{parse_line, RunEvent};

const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/stream.jsonl"
));

fn events() -> Vec<RunEvent> {
    FIXTURE.lines().flat_map(parse_line).collect()
}

#[test]
fn the_run_starts_with_its_session_and_model() {
    let first = events().into_iter().next().unwrap();
    assert_eq!(
        first,
        RunEvent::Started {
            session_id: "b0ceb2e4-9e94-4a98-8822-008ee87e297f".into(),
            model: "claude-haiku-4-5-20251001".into(),
        }
    );
}

#[test]
fn tool_calls_and_their_results_pair_by_id() {
    let events = events();
    let calls: Vec<(&str, &str)> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ToolCall { id, summary, .. } => Some((id.as_str(), summary.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].1, "Read tmp/wt/Cargo.toml");
    assert!(calls[1].1.starts_with("Asked to edit"), "{}", calls[1].1);

    let results: Vec<(&str, bool, &str)> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ToolResult { id, ok, summary } => Some((id.as_str(), *ok, summary.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].0, calls[0].0);
    assert!(!results[0].1, "a missing file is an error result");
    assert!(results[0].2.starts_with("File does not exist"));
    // The denied write: the result says so in two words, not a paragraph.
    assert_eq!(results[1], (calls[1].0, false, "not allowed"));
    assert!(results[2].1);
    assert_eq!(results[2].2, "");
}

#[test]
fn the_agent_text_arrives_per_message_and_thinking_is_dropped() {
    let texts: Vec<String> = events()
        .into_iter()
        .filter_map(|e| match e {
            RunEvent::Text(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), 3);
    assert_eq!(texts[0], "Hello! Now creating x.txt.");
    assert!(texts[2].starts_with("Done!"));
}

#[test]
fn the_result_line_ends_the_run_with_cost_and_time() {
    let events = events();
    assert_eq!(
        events.get(events.len() - 2),
        Some(&RunEvent::Usage {
            input_tokens: Some(34),
            output_tokens: Some(1163),
            cached_input_tokens: Some(152655),
        })
    );
    let last = events.last().cloned().unwrap();
    match last {
        RunEvent::Finished {
            session_id,
            cost_usd,
            duration_ms,
        } => {
            assert_eq!(session_id, "b0ceb2e4-9e94-4a98-8822-008ee87e297f");
            assert!((cost_usd.unwrap() - 0.0425).abs() < 0.001);
            assert_eq!(duration_ms, 14418);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, RunEvent::Finished { .. } | RunEvent::Failed { .. }))
            .count(),
        1
    );
}
