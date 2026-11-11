//! Spawn and cancel, against a shell script standing in for the agent.

use std::path::PathBuf;
use std::time::Duration;

use perch_agent::{
    Backend, Effort, ReviewEffort, ReviewSpec, Run, RunEvent, RunFailureKind, RunOutcome, RunSpec,
    Transcript,
};

/// Writes a script for a run to spawn, and returns once it can be.
///
/// On Linux a file open for writing can't be executed ("Text file busy"),
/// and a process another test forks while the script is being written
/// holds the write handle until it execs. So the script is run once with
/// `PERCH_FAKE_PROBE` set, which ends it at once, until that works.
fn fake_agent(dir: &std::path::Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-claude");
    std::fs::write(
        &path,
        format!("#!/bin/sh\n[ -n \"$PERCH_FAKE_PROBE\" ] && exit 0\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..200 {
        match std::process::Command::new(&path)
            .env("PERCH_FAKE_PROBE", "1")
            .status()
        {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(5));
            }
            result => {
                result.unwrap();
                break;
            }
        }
    }
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
        mode: perch_agent::Mode::Ask,
        read_dirs: Vec::new(),
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
async fn a_review_keeps_the_raw_stream_and_stderr_when_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let report = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"ReportFindings","input":{"findings":[],"level":"low"}}]},"parent_tool_use_id":null,"session_id":"s1"}"#;
    let result = r#"{"type":"result","subtype":"success","session_id":"s1","total_cost_usd":0.01,"duration_ms":5}"#;
    let bin = fake_agent(
        tmp.path(),
        &format!("echo '{INIT}'\necho 'noise on stderr' >&2\necho '{report}'\necho '{result}'"),
    );
    // The spec insists on a checkout; a marker is enough for the fake.
    std::fs::write(tmp.path().join(".git"), "gitdir: nowhere").unwrap();
    let transcript = Transcript {
        stdout: tmp.path().join("attempt.review.stream.jsonl"),
        stderr: tmp.path().join("attempt.review.stderr.log"),
    };
    let spec = ReviewSpec {
        backend: Backend::ClaudeCode,
        binary: bin,
        model: "sonnet".into(),
        effort: ReviewEffort::Low,
        cwd: tmp.path().to_path_buf(),
        merge_base_ref: "refs/perch/merge-base".into(),
        merge_base_sha: "1".repeat(40),
        head_sha: "2".repeat(40),
        output_schema: None,
        instructions: None,
        discussion_dir: None,
        transcript: Some(transcript.clone()),
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn_review(spec, tx).unwrap();

    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    assert!(matches!(rx.recv().await, Some(RunEvent::ToolCall { .. })));
    assert!(matches!(
        rx.recv().await,
        Some(RunEvent::Report { tool, .. }) if tool == "ReportFindings"
    ));
    assert!(matches!(rx.recv().await, Some(RunEvent::Finished { .. })));
    assert!(matches!(run.wait().await, RunOutcome::Finished { .. }));

    let stdout = std::fs::read_to_string(&transcript.stdout).unwrap();
    assert_eq!(stdout, format!("{INIT}\n{report}\n{result}\n"));
    let stderr = std::fs::read_to_string(&transcript.stderr).unwrap();
    assert_eq!(stderr, "noise on stderr\n");
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

#[tokio::test]
async fn the_agent_does_not_inherit_secrets() {
    let tmp = tempfile::tempdir().unwrap();
    let env_file = tmp.path().join("env.txt");
    let result = r#"{"type":"result","subtype":"success","session_id":"s1","duration_ms":5}"#;
    let bin = fake_agent(
        tmp.path(),
        &format!(
            "env > '{}'\necho '{INIT}'\necho '{result}'",
            env_file.display()
        ),
    );
    // Set in this process, under names of this test's own so nothing else
    // in the process reads them; the rules for the well-known names are
    // pinned by the unit tests. Only one test touches the environment.
    let scrubbed = [
        ("PERCH_TEST_NPM_TOKEN", "npm_secret"),
        ("PERCH_TEST_WEBHOOK_SECRET", "hook_secret"),
        ("PERCH_TEST_SIGNING_KEY", "signing_secret"),
        ("PERCH_TEST_DB_PASSWORD", "db_secret"),
    ];
    let kept = [
        ("PERCH_TEST_PLAIN", "plain"),
        ("PERCH_TEST_KEYBOARD", "qwerty"),
        ("PERCH_TEST_TOKEN_FILE", "/tmp/not-a-secret"),
    ];
    for (name, value) in scrubbed.iter().chain(kept.iter()) {
        std::env::set_var(name, value);
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(spec(bin, tmp.path().to_path_buf()), tx).unwrap();
    assert!(matches!(rx.recv().await, Some(RunEvent::Started { .. })));
    assert!(matches!(run.wait().await, RunOutcome::Finished { .. }));
    for (name, _) in scrubbed.iter().chain(kept.iter()) {
        std::env::remove_var(name);
    }

    let seen = std::fs::read_to_string(&env_file).unwrap();
    let names: Vec<&str> = seen
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .collect();
    let values: Vec<&str> = seen
        .lines()
        .filter_map(|line| line.split_once('=').map(|(_, value)| value))
        .collect();
    for (name, value) in scrubbed {
        assert!(!names.contains(&name), "{name} reached the agent:\n{seen}");
        assert!(
            !values.contains(&value),
            "{value} reached the agent:\n{seen}"
        );
    }
    for (name, value) in kept {
        assert!(
            seen.lines().any(|line| line == format!("{name}={value}")),
            "{name} was lost:\n{seen}"
        );
    }
    assert!(
        seen.contains("PATH="),
        "the rest of the environment is inherited"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cancel_hard_kills_a_stubborn_descendant_and_reaps_the_run() {
    let tmp = tempfile::tempdir().unwrap();
    let pid_file = tmp.path().join("descendant.pid");
    // The descendant writes its own pid once it ignores SIGTERM. Written by
    // the parent from `$!`, the pid could arrive before the trap, and a
    // cancel in that gap ended the run without needing SIGKILL.
    let body = format!(
        "echo '{INIT}'\nsh -c \"trap '' TERM; echo \\$\\$ > '{}'; while :; do sleep 1; done\" &\nwait",
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
        .args(["--exact", "run::watchdog_fixture_parent", "--nocapture"])
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
    let bin = fake_agent(
        tmp.path(),
        r#"read l; echo '{"id":1,"result":{}}'
read l
read l; echo '{"id":2,"result":{"thread":{"id":"01a0b584-d485-7d52-84b0-231e5b9b82d4"}}}'
read l; echo '{"id":3,"result":{"turn":{"id":"tu-1"}}}'
echo '{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":1,"outputTokens":1}}}}'
sleep 0.2
echo '{"method":"turn/completed","params":{"turn":{"id":"tu-1","status":"completed"}}}'
read eof || true"#,
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

fn work_spec(backend: Backend, binary: PathBuf, cwd: PathBuf) -> RunSpec {
    RunSpec {
        backend,
        mode: perch_agent::Mode::Work(perch_agent::WorkGrant::default()),
        ..spec(binary, cwd)
    }
}

/// Collect events until an approval arrives, then answer it and collect
/// the rest.
async fn answer_first_approval(
    run: &Run,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<RunEvent>,
    decision: perch_agent::Decision,
) -> (perch_agent::ApprovalRequest, Vec<RunEvent>) {
    let mut request = None;
    let mut rest = Vec::new();
    while let Some(event) = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the run went quiet")
    {
        match event {
            RunEvent::Approval { id, request: asked } if request.is_none() => {
                request = Some(asked);
                assert!(run.answer(&id, decision.clone()));
            }
            other => rest.push(other),
        }
    }
    (request.expect("the run asked"), rest)
}

#[tokio::test]
async fn a_claude_work_run_asks_its_host_and_is_answered_on_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("wrote");
    let body = format!(
        r#"read init
read user
printf '%s\n%s\n' "$init" "$user" > {record}
echo '{{"type":"system","subtype":"init","session_id":"s1","model":"m"}}'
echo '{{"type":"control_request","request_id":"r1","request":{{"subtype":"can_use_tool","tool_name":"SandboxNetworkAccess","input":{{"host":"example.com"}}}}}}'
read answer
printf '%s\n' "$answer" >> {record}
echo '{{"type":"result","subtype":"success","result":"Done.","session_id":"s1"}}'
read eof || true
"#,
        record = record.display()
    );
    let binary = fake_agent(dir.path(), &body);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(
        work_spec(Backend::ClaudeCode, binary, dir.path().to_path_buf()),
        tx,
    )
    .unwrap();

    let (request, rest) = answer_first_approval(
        &run,
        &mut rx,
        perch_agent::Decision::deny("The reader said no"),
    )
    .await;

    assert_eq!(request.kind, perch_agent::ApprovalKind::Host);
    assert_eq!(request.subject, "example.com");
    assert!(
        matches!(rest.last(), Some(RunEvent::Finished { .. })),
        "{rest:?}"
    );
    assert!(matches!(run.wait().await, RunOutcome::Finished { .. }));
    let wrote = std::fs::read_to_string(&record).unwrap();
    let lines: Vec<&str> = wrote.lines().collect();
    assert!(lines[0].contains("\"subtype\":\"initialize\""), "{wrote}");
    assert!(
        lines[1].contains("\"content\":\"hi\""),
        "the prompt went on stdin: {wrote}"
    );
    assert!(lines[2].contains("\"behavior\":\"deny\""), "{wrote}");
    assert!(lines[2].contains("The reader said no"), "{wrote}");
}

#[tokio::test]
async fn a_codex_work_run_is_an_app_server_turn_whose_approval_is_answered() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("wrote");
    let body = format!(
        r#"read l; echo '{{"id":1,"result":{{"userAgent":"x"}}}}'
read l
read l; printf '%s\n' "$l" > {record}
echo '{{"id":2,"result":{{"thread":{{"id":"th-1"}}}}}}'
read l; printf '%s\n' "$l" >> {record}
echo '{{"id":3,"result":{{"turn":{{"id":"tu-1"}}}}}}'
echo '{{"method":"item/started","params":{{"item":{{"type":"commandExecution","id":"c1","command":"/bin/zsh -lc \"curl https://example.com\""}}}}}}'
echo '{{"method":"item/commandExecution/requestApproval","id":5,"params":{{"command":"/bin/zsh -lc \"curl https://example.com\"","networkApprovalContext":{{"host":"example.com","protocol":"https"}}}}}}'
read answer; printf '%s\n' "$answer" >> {record}
echo '{{"method":"item/completed","params":{{"item":{{"type":"commandExecution","id":"c1","command":"x","exitCode":0}}}}}}'
echo '{{"method":"item/completed","params":{{"item":{{"type":"agentMessage","id":"m1","text":"Fetched."}}}}}}'
echo '{{"method":"turn/completed","params":{{"turn":{{"id":"tu-1","status":"completed"}}}}}}'
read eof || true
"#,
        record = record.display()
    );
    let binary = fake_agent(dir.path(), &body);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(
        work_spec(Backend::Codex, binary, dir.path().to_path_buf()),
        tx,
    )
    .unwrap();

    let (request, rest) = answer_first_approval(&run, &mut rx, perch_agent::Decision::Allow).await;

    assert_eq!(request.kind, perch_agent::ApprovalKind::Host);
    assert!(
        matches!(&rest[0], RunEvent::Started { session_id, .. } if session_id == "th-1"),
        "{rest:?}"
    );
    assert!(rest
        .iter()
        .any(|e| matches!(e, RunEvent::Text(t) if t == "Fetched.")));
    assert!(
        matches!(rest.last(), Some(RunEvent::Finished { session_id, .. }) if session_id == "th-1")
    );
    assert!(matches!(run.wait().await, RunOutcome::Finished { .. }));
    let wrote = std::fs::read_to_string(&record).unwrap();
    let lines: Vec<&str> = wrote.lines().collect();
    assert!(lines[0].contains("\"method\":\"thread/start\""), "{wrote}");
    assert!(lines[1].contains("\"method\":\"turn/start\""), "{wrote}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(lines[2]).unwrap(),
        serde_json::json!({"jsonrpc":"2.0","id":5,"result":{"decision":"accept"}})
    );
}

#[tokio::test]
async fn stopping_a_run_that_waits_for_an_answer_stops_it() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"read init
read user
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m"}'
echo '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"x","dangerouslyDisableSandbox":true}}}'
sleep 30
"#;
    let binary = fake_agent(dir.path(), body);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(
        work_spec(Backend::ClaudeCode, binary, dir.path().to_path_buf()),
        tx,
    )
    .unwrap();
    loop {
        match rx.recv().await {
            Some(RunEvent::Approval { .. }) => break,
            Some(_) => continue,
            None => panic!("no approval"),
        }
    }
    let outcome = tokio::time::timeout(Duration::from_secs(10), run.cancel_and_wait())
        .await
        .expect("the stop went through");
    assert!(matches!(outcome, RunOutcome::Stopped { .. }), "{outcome:?}");
    assert!(
        !run.answer("r1", perch_agent::Decision::Allow),
        "nothing to answer now"
    );
}

/// Collect events until one matches `at`, steer with `text`, and collect
/// the rest.
async fn steer_at(
    run: &Run,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<RunEvent>,
    at: impl Fn(&RunEvent) -> bool,
    text: &str,
) -> Vec<RunEvent> {
    let mut events = Vec::new();
    let mut steered = false;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the run went quiet")
    {
        if !steered && at(&event) {
            steered = true;
            assert!(run.steer(text));
        }
        events.push(event);
    }
    assert!(steered, "{events:?}");
    events
}

fn steered_at(events: &[RunEvent]) -> usize {
    events
        .iter()
        .position(|e| matches!(e, RunEvent::Steered(t) if t == "Skip the tests"))
        .unwrap_or_else(|| panic!("never steered: {events:?}"))
}

#[tokio::test]
async fn a_claude_question_is_steered_between_its_steps() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("wrote");
    let body = format!(
        r#"read init
read user
echo '{{"type":"system","subtype":"init","session_id":"s1","model":"m"}}'
echo '{{"type":"user","message":{{"role":"user","content":"hi"}},"parent_tool_use_id":null,"isReplay":true}}'
echo '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{"command":"cargo test"}}}}]}},"parent_tool_use_id":null}}'
read steer
printf '%s\n' "$steer" > {record}
echo '{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":"ok"}}]}},"parent_tool_use_id":null}}'
echo '{{"type":"user","message":{{"role":"user","content":"Skip the tests"}},"parent_tool_use_id":null,"isReplay":true}}'
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Skipped."}}]}},"parent_tool_use_id":null}}'
echo '{{"type":"result","subtype":"success","result":"Skipped.","session_id":"s1"}}'
read eof || true
"#,
        record = record.display()
    );
    let binary = fake_agent(dir.path(), &body);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(spec(binary, dir.path().to_path_buf()), tx).unwrap();

    let events = steer_at(
        &run,
        &mut rx,
        |e| matches!(e, RunEvent::ToolCall { .. }),
        "Skip the tests",
    )
    .await;
    let at = steered_at(&events);
    assert!(
        matches!(events[at - 1], RunEvent::ToolResult { .. }),
        "it lands after the step: {events:?}"
    );
    assert!(
        matches!(events[at + 1], RunEvent::Text(ref t) if t == "Skipped."),
        "{events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, RunEvent::Steered(_)))
            .count(),
        1,
        "not the prompt"
    );
    assert!(matches!(events.last(), Some(RunEvent::Finished { .. })));
    let wrote: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&record).unwrap().trim()).unwrap();
    assert_eq!(wrote["type"], "user");
    assert_eq!(wrote["message"]["content"], "Skip the tests");
    assert!(!run.steer("more"), "a finished run takes nothing");
}

#[tokio::test]
async fn a_claude_steer_the_turn_ended_before_reading_is_another_turn_of_the_run() {
    let dir = tempfile::tempdir().unwrap();
    // The first result comes after the steer was written but before it
    // was read. Stdin must stay open for the turn that reads it, or the
    // CLI would exit on EOF before it.
    let body = r#"read init
read user
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]},"parent_tool_use_id":null}'
read steer
echo '{"type":"result","subtype":"success","result":"Done.","session_id":"s1"}'
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m"}'
echo '{"type":"user","message":{"role":"user","content":"Skip the tests"},"parent_tool_use_id":null,"isReplay":true}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Skipped."}]},"parent_tool_use_id":null}'
echo '{"type":"result","subtype":"success","result":"Skipped.","session_id":"s1"}'
read eof || true
"#;
    let binary = fake_agent(dir.path(), body);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = Run::spawn(spec(binary, dir.path().to_path_buf()), tx).unwrap();
    let events = steer_at(
        &run,
        &mut rx,
        |e| matches!(e, RunEvent::Text(_)),
        "Skip the tests",
    )
    .await;

    let at = steered_at(&events);
    assert!(
        matches!(events[at + 1], RunEvent::Text(ref t) if t == "Skipped."),
        "{events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, RunEvent::Started { .. }))
            .count(),
        1,
        "the run starts once: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, RunEvent::Finished { .. }))
            .count(),
        1,
        "the first turn's result is not the run's: {events:?}"
    );
    assert!(matches!(events.last(), Some(RunEvent::Finished { .. })));
    assert!(matches!(run.wait().await, RunOutcome::Finished { .. }));
}

#[tokio::test]
async fn a_codex_steer_goes_to_its_turn_or_starts_the_next() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("wrote");
    let body = format!(
        r#"read l; echo '{{"id":1,"result":{{}}}}'
read l
read l; echo '{{"id":2,"result":{{"thread":{{"id":"th-1"}}}}}}'
read l; echo '{{"id":3,"result":{{"turn":{{"id":"tu-1"}}}}}}'
echo '{{"method":"item/started","params":{{"item":{{"type":"commandExecution","id":"c1","command":"cargo test"}}}}}}'
read steer; printf '%s\n' "$steer" > {record}
echo '{{"method":"item/completed","params":{{"item":{{"type":"commandExecution","id":"c1","command":"cargo test","exitCode":0}}}}}}'
echo '{{"method":"turn/completed","params":{{"turn":{{"id":"tu-1","status":"completed"}}}}}}'
echo '{{"id":100,"error":{{"code":-32600,"message":"no active turn to steer"}}}}'
read next; printf '%s\n' "$next" >> {record}
echo '{{"id":101,"result":{{"turn":{{"id":"tu-2"}}}}}}'
echo '{{"method":"item/completed","params":{{"item":{{"type":"userMessage","id":"u2","content":[{{"type":"text","text":"Skip the tests"}}]}}}}}}'
echo '{{"method":"item/completed","params":{{"item":{{"type":"agentMessage","id":"m2","text":"Skipped."}}}}}}'
echo '{{"method":"turn/completed","params":{{"turn":{{"id":"tu-2","status":"completed"}}}}}}'
read eof || true
"#,
        record = record.display()
    );
    let binary = fake_agent(dir.path(), &body);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut spec = spec(binary, dir.path().to_path_buf());
    spec.backend = Backend::Codex;
    let run = Run::spawn(spec, tx).unwrap();

    let events = steer_at(
        &run,
        &mut rx,
        |e| matches!(e, RunEvent::ToolCall { .. }),
        "Skip the tests",
    )
    .await;
    let at = steered_at(&events);
    assert!(
        matches!(events[at + 1], RunEvent::Text(ref t) if t == "Skipped."),
        "{events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, RunEvent::Finished { .. }))
            .count(),
        1,
        "{events:?}"
    );
    assert!(matches!(events.last(), Some(RunEvent::Finished { .. })));
    let wrote = std::fs::read_to_string(&record).unwrap();
    let lines: Vec<serde_json::Value> = wrote
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["method"], "turn/steer");
    assert_eq!(lines[0]["params"]["expectedTurnId"], "tu-1");
    assert_eq!(lines[0]["params"]["input"][0]["text"], "Skip the tests");
    assert_eq!(
        lines[1]["method"], "turn/start",
        "the steer missed its turn: {wrote}"
    );
    assert_eq!(lines[1]["params"]["input"][0]["text"], "Skip the tests");
}
