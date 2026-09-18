//! The workflow run lookup against a local server, to pin the request.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::{GitHub, SubjectState, WorkflowRunQuery};

fn read_path(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let path = start.split_whitespace().nth(1)?.to_string();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim_end().is_empty() {
            break;
        }
    }
    Some(path)
}

fn respond(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Serves one page of runs for any path, and remembers the paths asked for.
fn spawn(body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let seen = paths.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(path) = read_path(&mut stream) else {
                continue;
            };
            seen.lock().unwrap().push(path);
            respond(&mut stream, body);
        }
    });
    (format!("http://{addr}"), paths)
}

const RUNS: &str = r#"{"total_count":2,"workflow_runs":[
  {"id":35035566266,"name":"Hourly DST","run_number":912,"status":"completed","conclusion":"failure",
   "head_branch":"main","html_url":"https://github.com/slatedb/slatedb/actions/runs/35035566266",
   "created_at":"2026-09-15T23:25:07Z","updated_at":"2026-09-15T23:45:08Z",
   "actor":{"login":"criccomini","avatar_url":"https://avatars/1"}},
  {"id":35030000000,"name":"Hourly DST","run_number":911,"status":"completed","conclusion":"success",
   "head_branch":"main","html_url":"https://github.com/slatedb/slatedb/actions/runs/35030000000",
   "created_at":"2026-09-15T20:01:11Z","updated_at":"2026-09-15T20:14:21Z",
   "actor":{"login":"criccomini","avatar_url":"https://avatars/1"}}
]}"#;

fn query() -> WorkflowRunQuery {
    WorkflowRunQuery {
        repo_full_name: "slatedb/slatedb".into(),
        workflow: "Hourly DST".into(),
        branch: "main".into(),
        at: "2026-09-15T23:45:29Z".parse().unwrap(),
    }
}

#[tokio::test]
async fn the_run_is_asked_for_by_branch_and_window_and_found_by_name_and_time() {
    let (base, paths) = spawn(RUNS);
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    let detail = gh
        .fetch_workflow_run_detail(&query())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.reference(), "Hourly DST #912");
    assert_eq!(detail.state, SubjectState::RunFailure);
    assert_eq!(
        detail.html_url.as_deref(),
        Some("https://github.com/slatedb/slatedb/actions/runs/35035566266")
    );

    let paths = paths.lock().unwrap();
    assert_eq!(paths.len(), 1, "one request, no paging: {paths:?}");
    assert_eq!(
        paths[0],
        "/repos/slatedb/slatedb/actions/runs?branch=main\
         &created=2026-09-08T23:45:29Z..2026-09-15T23:45:29Z&per_page=100"
    );
}

#[tokio::test]
async fn a_branch_with_no_fitting_run_is_not_an_error() {
    let (base, _) = spawn(r#"{"total_count":0,"workflow_runs":[]}"#);
    let gh = GitHub::with_base_uri("t", &base).unwrap();
    assert_eq!(gh.fetch_workflow_run_detail(&query()).await.unwrap(), None);
}
