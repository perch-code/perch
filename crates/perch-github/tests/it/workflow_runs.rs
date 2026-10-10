//! The workflow run listings against a local server, to pin the requests.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::GitHub;

use crate::support::read_path;

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

#[tokio::test]
async fn the_runs_for_a_branch_are_asked_for_by_branch_and_days() {
    let (base, paths) = spawn(RUNS);
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    let runs = gh
        .list_workflow_runs_for_branch("slatedb/slatedb", "feature/dst", "2026-09-08..2026-09-15")
        .await
        .unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].run_number, 912);

    let paths = paths.lock().unwrap();
    assert_eq!(
        paths.as_slice(),
        ["/repos/slatedb/slatedb/actions/runs?branch=feature%2Fdst&created=2026-09-08..2026-09-15&per_page=100"]
    );
}

#[tokio::test]
async fn the_newest_runs_are_asked_for_with_no_filter() {
    let (base, paths) = spawn(RUNS);
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    let runs = gh
        .list_recent_workflow_runs("slatedb/slatedb")
        .await
        .unwrap();
    assert_eq!(runs.len(), 2);

    let paths = paths.lock().unwrap();
    assert_eq!(
        paths.as_slice(),
        ["/repos/slatedb/slatedb/actions/runs?per_page=100"]
    );
}
