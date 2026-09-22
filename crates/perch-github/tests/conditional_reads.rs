//! REST reads ask with the ETag they last saw, and a 304 gives back the
//! body that tag came with.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::GitHub;

#[derive(Debug)]
struct Request {
    path: String,
    if_none_match: Option<String>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let path = start.split_whitespace().nth(1)?.to_string();
    let mut if_none_match = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim_end().is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            if name.trim().eq_ignore_ascii_case("if-none-match") {
                if_none_match = Some(value.trim().to_string());
            }
        }
    }
    Some(Request {
        path,
        if_none_match,
    })
}

/// A status line, an ETag to send or none, and a body.
type Response = (&'static str, Option<&'static str>, &'static str);

/// Serve exactly the scripted responses, in order.
fn spawn(responses: Vec<Response>) -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for (status, etag, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream).unwrap();
            seen.lock().unwrap().push(request);
            let etag = etag.map(|e| format!("ETag: {e}\r\n")).unwrap_or_default();
            let wire = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{etag}Connection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(wire.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
    });
    (format!("http://{addr}"), requests)
}

const RUNS: &str = r#"{"total_count":1,"workflow_runs":[
  {"id":1,"name":"CI","run_number":7,"status":"in_progress","conclusion":null,
   "head_branch":"main","html_url":"https://github.com/a/b/actions/runs/1",
   "created_at":"2026-09-15T23:25:07Z","updated_at":"2026-09-15T23:45:08Z",
   "actor":{"login":"octo","avatar_url":"https://avatars/1"}}
]}"#;

const RUNS_DONE: &str = r#"{"total_count":1,"workflow_runs":[
  {"id":1,"name":"CI","run_number":7,"status":"completed","conclusion":"success",
   "head_branch":"main","html_url":"https://github.com/a/b/actions/runs/1",
   "created_at":"2026-09-15T23:25:07Z","updated_at":"2026-09-15T23:50:00Z",
   "actor":{"login":"octo","avatar_url":"https://avatars/1"}}
]}"#;

#[tokio::test]
async fn an_unchanged_read_is_answered_from_the_body_its_tag_came_with() {
    let (base, requests) = spawn(vec![
        ("200 OK", Some(r#"W/"one""#), RUNS),
        ("304 Not Modified", None, ""),
        ("200 OK", Some(r#"W/"two""#), RUNS_DONE),
        ("304 Not Modified", None, ""),
    ]);
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    let first = gh.list_workflow_runs_for_sha("a/b", "abc").await.unwrap();
    let second = gh.list_workflow_runs_for_sha("a/b", "abc").await.unwrap();
    assert_eq!(first[0].status, "in_progress");
    assert_eq!(second[0].status, "in_progress");

    let third = gh.list_workflow_runs_for_sha("a/b", "abc").await.unwrap();
    assert_eq!(third[0].status, "completed");
    let fourth = gh.list_workflow_runs_for_sha("a/b", "abc").await.unwrap();
    assert_eq!(fourth[0].conclusion.as_deref(), Some("success"));

    let requests = requests.lock().unwrap();
    let tags: Vec<_> = requests
        .iter()
        .map(|r| r.if_none_match.as_deref())
        .collect();
    assert_eq!(
        tags,
        [
            None,
            Some(r#"W/"one""#),
            Some(r#"W/"one""#),
            Some(r#"W/"two""#)
        ]
    );
}

#[tokio::test]
async fn each_page_of_a_list_asks_with_its_own_tag() {
    let comment = r#"[{"id":5,"body":"hi","created_at":"2026-09-15T23:25:07Z"}]"#;
    let (base, requests) = spawn(vec![
        ("200 OK", Some(r#""c1""#), comment),
        ("304 Not Modified", None, ""),
    ]);
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    let first = gh.fetch_issue_comments("a/b", 9).await.unwrap();
    let second = gh.fetch_issue_comments("a/b", 9).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(second[0].body, "hi");

    let requests = requests.lock().unwrap();
    assert_eq!(
        requests[1].path,
        "/repos/a/b/issues/9/comments?per_page=100&page=1"
    );
    assert_eq!(requests[1].if_none_match.as_deref(), Some(r#""c1""#));
}

#[tokio::test]
async fn a_read_that_came_without_a_tag_is_asked_for_whole_next_time() {
    let (base, requests) = spawn(vec![("200 OK", None, RUNS), ("200 OK", None, RUNS)]);
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    gh.list_workflow_runs_for_sha("a/b", "abc").await.unwrap();
    gh.list_workflow_runs_for_sha("a/b", "abc").await.unwrap();

    let requests = requests.lock().unwrap();
    assert!(requests.iter().all(|r| r.if_none_match.is_none()));
}
