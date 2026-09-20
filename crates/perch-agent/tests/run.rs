//! Spawn and cancel, against a shell script standing in for the agent.

use std::path::PathBuf;
use std::time::Duration;

use perch_agent::{Backend, Effort, Run, RunEvent, RunFailureKind, RunOutcome, RunSpec};

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

#[cfg(unix)]
const WATCHDOG_FIXTURE: &str = "PERCH_AGENT_WATCHDOG_FIXTURE";

/// A subprocess-only fixture. The outer test kills this test runner with
/// SIGKILL, leaving no Rust destructor or Tokio shutdown path available.
#[cfg(unix)]
#[test]
fn watchdog_fixture_parent() {
    if std::env::var_os(WATCHDOG_FIXTURE).is_none() {
        return;
    }
    let binary = PathBuf::from(std::env::var_os("PERCH_AGENT_FIXTURE_BINARY").unwrap());
    let ready = PathBuf::from(std::env::var_os("PERCH_AGENT_FIXTURE_READY").unwrap());
    let cwd = binary.parent().unwrap().to_path_buf();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let _run = Run::spawn(spec(binary, cwd), tx).unwrap();
            assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
            std::fs::write(ready, b"ready").unwrap();
            let _keep_receiver_alive = rx;
            std::future::pending::<()>().await;
        });
}

#[tokio::test]
async fn cancel_ends_the_child_and_yields_stopped() {
    let tmp = tempfile::tempdir().unwrap();
    // Keep this fixture to one process so a graceful TERM has no orphaned
    // zombie whose reaping policy depends on the test host's PID 1.
    let bin = fake_agent(
        tmp.path(),
        &format!("echo '{INIT}'\ntrap 'exit 0' TERM\nwhile :; do :; done"),
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();

    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    run.cancel();
    let end = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("ended in time");
    assert_eq!(end, Some(RunEvent::Stopped { forced: false }));
    assert_eq!(run.wait().await, RunOutcome::Stopped { forced: false });
    assert_eq!(rx.recv().await, None, "nothing after the end");
}

#[tokio::test]
async fn a_process_that_dies_is_a_failure_with_its_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = fake_agent(
        tmp.path(),
        &format!("echo '{INIT}'\necho 'not logged in' >&2\nexit 3"),
    );
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
    let run = Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();

    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    assert!(matches!(rx.recv().await, Some(RunEvent::Finished { .. })));
    assert!(matches!(run.wait().await, RunOutcome::Finished { .. }));
    assert_eq!(
        run.wait().await,
        run.clone().wait().await,
        "every handle sees the same outcome"
    );
    assert_eq!(rx.recv().await, None);
}

#[tokio::test]
async fn completion_is_not_published_until_the_process_is_reaped() {
    let tmp = tempfile::tempdir().unwrap();
    let result = r#"{"type":"result","subtype":"success","session_id":"s1","total_cost_usd":0.01,"duration_ms":5}"#;
    let bin = fake_agent(
        tmp.path(),
        &format!("echo '{INIT}'\necho '{result}'\nsleep 0.2"),
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();

    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), rx.recv())
            .await
            .is_err(),
        "the protocol result alone is not completion"
    );
    assert!(matches!(run.wait().await, RunOutcome::Finished { .. }));
    assert!(matches!(rx.recv().await, Some(RunEvent::Finished { .. })));
}

#[cfg(unix)]
#[tokio::test]
async fn cancel_hard_kills_a_stubborn_descendant_and_reaps_the_run() {
    let tmp = tempfile::tempdir().unwrap();
    let pid_file = tmp.path().join("descendant.pid");
    let body = format!(
        "echo '{INIT}'\n(trap '' TERM; while :; do sleep 1; done) &\necho $! > '{}'\nwait",
        pid_file.display()
    );
    let bin = fake_agent(tmp.path(), &body);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();

    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    let descendant = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = text.trim().parse::<i32>() {
                    break pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the fake agent wrote its descendant pid");

    assert_eq!(
        run.cancel_and_wait().await,
        RunOutcome::Stopped { forced: true }
    );
    assert_eq!(rx.recv().await, Some(RunEvent::Stopped { forced: true }));
    assert_eq!(rx.recv().await, None);

    let gone = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            // SAFETY: signal 0 only checks whether this fixture process is
            // still present; it does not change it.
            if unsafe { libc::kill(descendant, 0) } != 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "the stubborn descendant was left behind");
}

#[cfg(unix)]
#[tokio::test]
async fn hard_parent_death_watchdog_kills_the_agent_descendant() {
    let tmp = tempfile::tempdir().unwrap();
    let pid_file = tmp.path().join("descendant.pid");
    let ready_file = tmp.path().join("runner.ready");
    let body = format!(
        "echo '{INIT}'\n(trap '' TERM; while :; do sleep 1; done) &\necho $! > '{}'\nwait",
        pid_file.display()
    );
    let binary = fake_agent(tmp.path(), &body);
    let mut parent = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "watchdog_fixture_parent", "--nocapture"])
        .env(WATCHDOG_FIXTURE, "1")
        .env("PERCH_AGENT_FIXTURE_BINARY", &binary)
        .env("PERCH_AGENT_FIXTURE_READY", &ready_file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let descendant = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if ready_file.exists() {
                if let Ok(text) = std::fs::read_to_string(&pid_file) {
                    if let Ok(pid) = text.trim().parse::<i32>() {
                        break pid;
                    }
                }
            }
            if let Some(status) = parent.try_wait().unwrap() {
                panic!("watchdog fixture exited before it was ready: {status}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the protected runner became ready");

    // SAFETY: this is the fixture process we just spawned. SIGKILL proves
    // cleanup does not depend on unwinding, Drop, or Tokio runtime shutdown.
    assert_eq!(unsafe { libc::kill(parent.id() as i32, libc::SIGKILL) }, 0);
    parent.wait().unwrap();

    let gone = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            // SAFETY: signal 0 only observes the known fixture descendant.
            if unsafe { libc::kill(descendant, 0) } != 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if gone.is_err() {
        // Avoid leaking the deliberately stubborn fixture when an assertion
        // fails while developing the watchdog.
        unsafe {
            libc::kill(descendant, libc::SIGKILL);
        }
    }
    assert!(
        gone.is_ok(),
        "the agent descendant survived its parent's SIGKILL"
    );
}

#[tokio::test]
async fn explicit_permission_denial_types_a_later_failed_result() {
    let tmp = tempfile::tempdir().unwrap();
    let denied = r#"{"type":"system","subtype":"permission_denied","tool_name":"Write"}"#;
    let failed = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Could not complete the task","session_id":"s1"}"#;
    let bin = fake_agent(
        tmp.path(),
        &format!("echo '{INIT}'\necho '{denied}'\necho '{failed}'"),
    );
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();

    assert_eq!(
        run.wait().await,
        RunOutcome::Failed {
            kind: RunFailureKind::PermissionDenied,
            message: "Could not complete the task".into(),
        }
    );
}

#[tokio::test]
async fn malformed_nonblank_json_is_a_typed_output_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = fake_agent(tmp.path(), &format!("echo '{INIT}'\necho '{{not-json'"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();

    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    match run.wait().await {
        RunOutcome::Failed {
            kind: RunFailureKind::Output,
            message,
        } => {
            assert!(message.contains("malformed JSON"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(rx.recv().await, Some(RunEvent::Failed { message }) if message.contains("malformed JSON"))
    );
}

#[tokio::test]
async fn valid_unknown_json_is_ignored_without_failing_the_stream() {
    let tmp = tempfile::tempdir().unwrap();
    let unknown = r#"{"type":"future.event","value":1}"#;
    let result = r#"{"type":"result","subtype":"success","session_id":"s1"}"#;
    let bin = fake_agent(
        tmp.path(),
        &format!("echo '{INIT}'\necho '{unknown}'\necho '{result}'"),
    );
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();

    assert!(matches!(run.wait().await, RunOutcome::Finished { .. }));
}

#[tokio::test]
async fn a_codex_run_ends_with_its_thread_and_the_time_it_took() {
    let tmp = tempfile::tempdir().unwrap();
    let thread = r#"{"type":"thread.started","thread_id":"01a0b584-d485-7d52-84b0-231e5b9b82d4"}"#;
    let done = r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;
    let bin = fake_agent(
        tmp.path(),
        &format!("echo '{thread}'\nsleep 0.2\necho '{done}'"),
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut spec = spec(bin, tmp.path().to_path_buf());
    spec.backend = Backend::Codex;
    Run::spawn(spec, tx).unwrap();

    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    assert_eq!(
        rx.recv().await,
        Some(RunEvent::Usage {
            input_tokens: Some(1),
            output_tokens: Some(1),
            cached_input_tokens: None,
        })
    );
    match rx.recv().await {
        Some(RunEvent::Finished {
            session_id,
            cost_usd,
            duration_ms,
        }) => {
            assert_eq!(session_id, "01a0b584-d485-7d52-84b0-231e5b9b82d4");
            assert_eq!(cost_usd, None);
            assert!(
                duration_ms >= 200,
                "the wall clock stands in: {duration_ms}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(rx.recv().await, None);
}

#[tokio::test]
async fn a_missing_binary_fails_to_spawn() {
    let tmp = tempfile::tempdir().unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = Run::spawn(spec(tmp.path().join("nope"), tmp.path().to_path_buf()), tx).unwrap_err();
    assert!(err.to_string().contains("nope"));
}
