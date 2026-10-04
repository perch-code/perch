//! Deleting and editing a comment against a scripted local server, and
//! the line a deletion leaves in the timeline.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::{CommentPlace, Error, GitHub, PullEventKind};

#[derive(Debug)]
struct Request {
    path: String,
    body: serde_json::Value,
}

struct Response {
    status: &'static str,
    body: String,
}

impl Response {
    fn ok(body: impl Into<String>) -> Self {
        Self {
            status: "200 OK",
            body: body.into(),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let path = start.split_whitespace().nth(1)?.to_string();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        path,
        body: serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    })
}

fn respond(stream: &mut TcpStream, response: Response) {
    let wire = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}",
        status = response.status,
        length = response.body.len(),
        body = response.body,
    );
    stream.write_all(wire.as_bytes()).unwrap();
    stream.flush().unwrap();
}

fn spawn(responses: Vec<Response>) -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for (stream, response) in listener.incoming().zip(responses) {
            let Ok(mut stream) = stream else { continue };
            let Some(request) = read_request(&mut stream) else {
                continue;
            };
            seen.lock().unwrap().push(request);
            respond(&mut stream, response);
        }
    });
    (format!("http://{address}"), requests)
}

#[tokio::test]
async fn each_place_is_deleted_by_its_own_mutation() {
    let (base, requests) = spawn(vec![
        Response::ok(r#"{"data":{"deleteIssueComment":{"clientMutationId":null}}}"#),
        Response::ok(r#"{"data":{"deletePullRequestReviewComment":{"clientMutationId":null}}}"#),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    github
        .delete_comment("IC_1", CommentPlace::Conversation)
        .await
        .unwrap();
    github
        .delete_comment("PRRC_2", CommentPlace::Line)
        .await
        .unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let query = |i: usize| requests[i].body["query"].as_str().unwrap().to_string();
    assert!(query(0).contains("deleteIssueComment(input: {id: $id})"));
    assert_eq!(requests[0].body["variables"]["id"], "IC_1");
    assert!(query(1).contains("deletePullRequestReviewComment(input: {id: $id})"));
    assert_eq!(requests[1].body["variables"]["id"], "PRRC_2");
    assert_eq!(requests[0].path, "/graphql");
}

#[tokio::test]
async fn each_place_is_edited_by_its_own_mutation_and_answers_the_kept_body() {
    let (base, requests) = spawn(vec![
        Response::ok(r#"{"data":{"updateIssueComment":{"issueComment":{"body":"fixed"}}}}"#),
        Response::ok(
            r#"{"data":{"updatePullRequestReviewComment":{"pullRequestReviewComment":{"body":"also fixed"}}}}"#,
        ),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let kept = github
        .update_comment("IC_1", CommentPlace::Conversation, "fixed")
        .await
        .unwrap();
    assert_eq!(kept, "fixed");
    let kept = github
        .update_comment("PRRC_2", CommentPlace::Line, "also fixed")
        .await
        .unwrap();
    assert_eq!(kept, "also fixed");

    let requests = requests.lock().unwrap();
    let query = |i: usize| requests[i].body["query"].as_str().unwrap().to_string();
    assert!(query(0).contains("updateIssueComment(input: {id: $id, body: $body})"));
    assert_eq!(requests[0].body["variables"]["id"], "IC_1");
    assert_eq!(requests[0].body["variables"]["body"], "fixed");
    assert!(query(1).contains(
        "updatePullRequestReviewComment(input: {pullRequestReviewCommentId: $id, body: $body})"
    ));
    assert_eq!(requests[1].body["variables"]["id"], "PRRC_2");
    assert_eq!(requests[1].body["variables"]["body"], "also fixed");
}

#[tokio::test]
async fn a_refused_edit_is_an_error_and_no_id_asks_nothing() {
    let (base, requests) = spawn(vec![Response::ok(
        r#"{"data":{"updateIssueComment":null},"errors":[{"message":"Resource not accessible by integration"}]}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let refused = github
        .update_comment("IC_1", CommentPlace::Conversation, "no")
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, Error::Graphql(message) if message.contains("not accessible")),
        "{refused:?}"
    );
    let none = github
        .update_comment("", CommentPlace::Line, "no")
        .await
        .unwrap_err();
    assert!(matches!(none, Error::Decode(_)), "{none:?}");
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_refused_deletion_is_an_error_and_no_id_asks_nothing() {
    let (base, requests) = spawn(vec![Response::ok(
        r#"{"data":{"deleteIssueComment":null},"errors":[{"message":"Resource not accessible by integration"}]}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let refused = github
        .delete_comment("IC_1", CommentPlace::Conversation)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, Error::Graphql(message) if message.contains("not accessible")),
        "{refused:?}"
    );
    let none = github
        .delete_comment("", CommentPlace::Line)
        .await
        .unwrap_err();
    assert!(matches!(none, Error::Decode(_)), "{none:?}");
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_deleted_comment_is_a_timeline_line_naming_whose_it_was() {
    let (base, requests) = spawn(vec![
        Response::ok(
            r#"[{"event":"comment_deleted","node_id":"CDE_1","actor":{"login":"criccomini"},"created_at":"2026-10-03T22:15:58Z"},
                {"event":"comment_deleted","node_id":"CDE_2","actor":{"login":"criccomini"},"created_at":"2026-10-03T22:16:30Z"}]"#,
        ),
        Response::ok(
            r#"{"data":{"nodes":[{"deletedCommentAuthor":{"login":"kvnloo"}},{"deletedCommentAuthor":null}]}}"#,
        ),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let events = github.fetch_timeline("a/b", 7, &[]).await.unwrap();
    let kinds: Vec<_> = events
        .iter()
        .map(|e| (e.actor.as_str(), e.kind.clone()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (
                "criccomini",
                PullEventKind::CommentDeleted {
                    author: "kvnloo".into()
                }
            ),
            (
                "criccomini",
                PullEventKind::CommentDeleted {
                    author: String::new()
                }
            ),
        ],
        "an author GitHub no longer has leaves the line unnamed"
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests[1].path, "/graphql");
    assert_eq!(
        requests[1].body["variables"]["ids"],
        serde_json::json!(["CDE_1", "CDE_2"])
    );
}

#[tokio::test]
async fn a_timeline_without_deletions_asks_graphql_nothing() {
    let (base, requests) = spawn(vec![Response::ok(
        r#"[{"event":"closed","actor":{"login":"chris"},"created_at":"2026-10-03T22:15:58Z"}]"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    let events = github.fetch_timeline("a/b", 7, &[]).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(requests.lock().unwrap().len(), 1);
}
