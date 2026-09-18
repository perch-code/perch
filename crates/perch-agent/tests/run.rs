//! Spawn and cancel, against a shell script standing in for the agent.

use std::path::PathBuf;
use std::time::Duration;

use perch_agent::{Backend, Effort, Run, RunEvent, RunSpec};

fn fake_agent(dir: &std::path::Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-claude");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn spec(binary: PathBuf, cwd: PathBuf) -> RunSpec {
    RunSpec {
        backend: Backend::ClaudeCode,
        binary,
        model: "sonnet".into(),
        effort: Effort::Low,
        cwd,
        prompt: "hi".into(),
        system: None,
        resume: None,
        allowed_tools: vec![],
    }
}

const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s1","model":"m"}"#;

#[tokio::test]
async fn cancel_ends_the_child_and_yields_stopped() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = fake_agent(tmp.path(), &format!("echo '{INIT}'\nsleep 30"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();

    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    run.cancel();
    let end = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("ended in time");
    assert_eq!(end, Some(RunEvent::Failed { message: "Stopped".into() }));
    assert_eq!(rx.recv().await, None, "nothing after the end");
}

#[tokio::test]
async fn a_process_that_dies_is_a_failure_with_its_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = fake_agent(tmp.path(), &format!("echo '{INIT}'\necho 'not logged in' >&2\nexit 3"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();

    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    match rx.recv().await {
        Some(RunEvent::Failed { message }) => {
            assert!(message.contains("exit status: 3"), "{message}");
            assert!(message.contains("not logged in"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_finished_run_sends_nothing_more() {
    let tmp = tempfile::tempdir().unwrap();
    let result = r#"{"type":"result","subtype":"success","session_id":"s1","total_cost_usd":0.01,"duration_ms":5}"#;
    let bin = fake_agent(tmp.path(), &format!("echo '{INIT}'\necho '{result}'"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();

    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    assert!(matches!(rx.recv().await, Some(RunEvent::Finished { .. })));
    assert_eq!(rx.recv().await, None);
}

#[tokio::test]
async fn a_missing_binary_fails_to_spawn() {
    let tmp = tempfile::tempdir().unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = Run::spawn(spec(tmp.path().join("nope"), tmp.path().to_path_buf()), tx).unwrap_err();
    assert!(err.to_string().contains("nope"));
}
