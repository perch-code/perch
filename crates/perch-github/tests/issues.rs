//! The issue fetch against a local server, to pin the mapping, the
//! pagination of comments, and the refusal of a pull request.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use perch_github::{Error, GitHub, ItemKind};

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

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn comment(id: usize) -> String {
    format!(
        r#"{{"id":{id},"user":{{"login":"u{id}"}},"body":"comment {id}","created_at":"2026-09-15T09:{:02}:00Z","html_url":"https://github.com/a/b/issues/42#issuecomment-{id}"}}"#,
        id % 60
    )
}

fn page(items: Vec<String>) -> String {
    format!("[{}]", items.join(","))
}

const ISSUE: &str = r#"{
    "number": 42,
    "title": "Fence the manifest",
    "body": "It races.",
    "state": "open",
    "state_reason": null,
    "user": { "login": "rohan", "avatar_url": "https://avatars/2" },
    "labels": [ { "name": "bug", "color": "d73a4a" } ],
    "assignees": [ { "login": "chris" } ],
    "comments": 150,
    "created_at": "2026-09-14T14:02:00Z",
    "updated_at": "2026-09-16T11:40:00Z"
}"#;

/// The same endpoint answering for a pull request: the shape of an issue
/// plus a `pull_request` key.
const PULL_AS_ISSUE: &str = r#"{
    "number": 7,
    "title": "A pull request",
    "body": "",
    "state": "open",
    "user": { "login": "rohan" },
    "comments": 0,
    "created_at": "2026-09-14T14:02:00Z",
    "updated_at": "2026-09-16T11:40:00Z",
    "pull_request": { "url": "https://api.github.com/repos/a/b/pulls/7" }
}"#;

/// Issue 42 with `comments` conversation comments a hundred a page, and
/// number 7 being a pull request.
fn spawn(comments: usize) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(path) = read_path(&mut stream) else {
                continue;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            let page_no: usize = path
                .split("&page=")
                .nth(1)
                .and_then(|p| p.split('&').next())
                .and_then(|p| p.parse().ok())
                .unwrap_or(1);
            if path.contains("/issues/42/comments") {
                let start = (page_no - 1) * 100;
                let end = (start + 100).min(comments);
                respond(
                    &mut stream,
                    "200 OK",
                    &page((start..end).map(|i| comment(i + 1)).collect()),
                );
            } else if path.contains("/issues/42") {
                respond(&mut stream, "200 OK", ISSUE);
            } else if path.contains("/issues/7") {
                respond(&mut stream, "200 OK", PULL_AS_ISSUE);
            } else {
                respond(&mut stream, "404 Not Found", r#"{"message":"Not Found"}"#);
            }
        }
    });
    (format!("http://{addr}"), requests)
}

#[tokio::test]
async fn an_issue_arrives_with_its_repository_filled_in() {
    let (base, requests) = spawn(150);
    let gh = GitHub::with_base_uri("token", &base).unwrap();

    let issue = gh.fetch_issue("a/b", 42).await.unwrap();

    assert_eq!(issue.number, 42);
    assert_eq!(issue.title, "Fence the manifest");
    assert_eq!(issue.body, "It races.");
    assert!(issue.is_open());
    assert_eq!(issue.author_login, "rohan");
    assert_eq!(issue.labels[0].name, "bug");
    assert_eq!(issue.assignees, vec!["chris"]);
    assert_eq!(issue.comments, 150);
    assert_eq!(
        issue.created_at,
        Some("2026-09-14T14:02:00Z".parse().unwrap())
    );
    assert_eq!(
        issue.repo_full_name, "a/b",
        "the repository comes from the caller"
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn every_page_of_the_comments_is_read() {
    let (base, requests) = spawn(150);
    let gh = GitHub::with_base_uri("token", &base).unwrap();

    let comments = gh.fetch_issue_comments("a/b", 42).await.unwrap();

    assert_eq!(comments.len(), 150, "two pages, the second short");
    assert_eq!(comments[0].user, "u1");
    assert_eq!(comments[149].body, "comment 150");
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_pull_request_is_refused_as_an_issue() {
    let (base, _) = spawn(0);
    let gh = GitHub::with_base_uri("token", &base).unwrap();

    let err = gh.fetch_issue("a/b", 7).await.unwrap_err();

    match err {
        Error::Decode(message) => assert_eq!(message, "a/b#7 is a pull request"),
        other => panic!("expected a decode error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_missing_issue_is_a_status_error() {
    let (base, _) = spawn(0);
    let gh = GitHub::with_base_uri("token", &base).unwrap();
    assert!(matches!(
        gh.fetch_issue("a/b", 999).await.unwrap_err(),
        Error::Status { status: 404, .. }
    ));
}

#[tokio::test]
async fn a_number_is_told_a_pull_request_or_an_issue() {
    let (base, _) = spawn(0);
    let gh = GitHub::with_base_uri("token", &base).unwrap();
    assert_eq!(gh.subject_kind("a/b", 42).await.unwrap(), ItemKind::Issue);
    assert_eq!(
        gh.subject_kind("a/b", 7).await.unwrap(),
        ItemKind::PullRequest
    );
    assert!(matches!(
        gh.subject_kind("a/b", 999).await.unwrap_err(),
        Error::Status { status: 404, .. }
    ));
}
