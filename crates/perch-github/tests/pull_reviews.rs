//! Pull-request review mutations against a scripted local server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::{Error, GitHub, PullReviewEvent, ReviewCommentDraft, Side};

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    body: String,
}

struct Response {
    status: &'static str,
    body: String,
}

impl Response {
    fn new(status: &'static str, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let mut start = start.split_whitespace();
    let method = start.next()?.to_string();
    let path = start.next()?.to_string();
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
        method,
        path,
        body: String::from_utf8_lossy(&body).into_owned(),
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

fn review(id: u64, state: &str, body: &str) -> String {
    serde_json::json!({
        "id": id,
        "user": { "login": "reviewer" },
        "body": body,
        "state": state,
        "submitted_at": "2026-09-20T18:00:00Z",
        "html_url": format!("https://github.com/a/b/pull/42#pullrequestreview-{id}"),
    })
    .to_string()
}

#[tokio::test]
async fn all_final_review_events_post_the_exact_commit_and_decode_the_review() {
    let (base, requests) = spawn(vec![
        Response::new("200 OK", review(1, "COMMENTED", "General note")),
        Response::new("200 OK", review(2, "APPROVED", "")),
        Response::new("200 OK", review(3, "CHANGES_REQUESTED", "Please fix this")),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let cases = [
        (PullReviewEvent::Comment, "General note", "COMMENTED"),
        (PullReviewEvent::Approve, "", "APPROVED"),
        (
            PullReviewEvent::RequestChanges,
            "Please fix this",
            "CHANGES_REQUESTED",
        ),
    ];
    for (event, body, state) in cases {
        let submitted = github
            .submit_pull_request_review("a/b", 42, "abc123", body, event, &[])
            .await
            .unwrap();
        assert_eq!(submitted.user, "reviewer");
        assert_eq!(submitted.body, body);
        assert_eq!(submitted.state, state);
        assert!(submitted.submitted_at.is_some());
    }

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for (request, (event, body)) in requests.iter().zip([
        ("COMMENT", "General note"),
        ("APPROVE", ""),
        ("REQUEST_CHANGES", "Please fix this"),
    ]) {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/repos/a/b/pulls/42/reviews");
        let json: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(json["commit_id"], "abc123");
        assert_eq!(json["body"], body);
        assert_eq!(json["event"], event);
        assert!(
            json.get("comments").is_none(),
            "a review with no comments on lines sends none"
        );
    }
}

#[tokio::test]
async fn required_bodies_and_commit_ids_are_validated_before_the_request() {
    // The one valid request consumes the one response. If either invalid
    // call reached the server, the valid approval would have no response.
    let (base, requests) = spawn(vec![Response::new("200 OK", review(7, "APPROVED", ""))]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    assert!(matches!(
        github
            .submit_pull_request_review("a/b", 42, "abc123", "  ", PullReviewEvent::Comment, &[])
            .await,
        Err(Error::Decode(message)) if message.contains("requires a body")
    ));
    assert!(matches!(
        github
            .submit_pull_request_review(
                "a/b",
                42,
                "abc123",
                "\n",
                PullReviewEvent::RequestChanges,
                &[],
            )
            .await,
        Err(Error::Decode(message)) if message.contains("requires a body")
    ));
    assert!(matches!(
        github
            .submit_pull_request_review("a/b", 42, " bad ", "", PullReviewEvent::Approve, &[])
            .await,
        Err(Error::Decode(message)) if message.contains("commit SHA")
    ));

    github
        .submit_pull_request_review("a/b", 42, "abc123", "", PullReviewEvent::Approve, &[])
        .await
        .unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn github_validation_failures_keep_their_status_and_body() {
    let (base, _) = spawn(vec![Response::new(
        "422 Unprocessable Entity",
        r#"{"message":"Can not approve your own pull request"}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    assert!(matches!(
        github
            .submit_pull_request_review("a/b", 42, "abc123", "", PullReviewEvent::Approve, &[])
            .await,
        Err(Error::Status { status: 422, body })
            if body.contains("Can not approve your own pull request")
    ));
}

fn review_comment(id: u64, reply_to: Option<u64>, body: &str) -> String {
    serde_json::json!({
        "id": id,
        "node_id": format!("PRRC_{id}"),
        "pull_request_review_id": null,
        "in_reply_to_id": reply_to,
        "user": { "login": "reviewer" },
        "body": body,
        "path": "src/x.rs",
        "line": 41,
        "side": "RIGHT",
        "diff_hunk": "@@ -40,3 +40,3 @@",
        "commit_id": "abc123",
        "original_commit_id": "abc123",
        "created_at": "2026-09-20T18:00:00Z",
        "html_url": format!("https://github.com/a/b/pull/42#discussion_r{id}"),
    })
    .to_string()
}

fn draft(path: &str, start_line: Option<u64>, line: u64, body: &str) -> ReviewCommentDraft {
    ReviewCommentDraft {
        path: path.into(),
        line,
        side: Side::Right,
        start_line,
        start_side: start_line.map(|_| Side::Right),
        body: body.into(),
    }
}

#[tokio::test]
async fn a_review_carries_its_comments_on_lines_and_needs_no_summary_with_them() {
    let (base, requests) = spawn(vec![Response::new(
        "200 OK",
        review(4, "COMMENTED", ""),
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let comments = [
        draft("src/x.rs", None, 41, "One line"),
        draft("src/y.rs", Some(7), 9, "Three lines"),
    ];
    let submitted = github
        .submit_pull_request_review("a/b", 42, "abc123", "", PullReviewEvent::Comment, &comments)
        .await
        .unwrap();
    assert_eq!(submitted.id, 4);

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let json: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(json["event"], "COMMENT");
    assert_eq!(json["body"], "");
    let sent = json["comments"].as_array().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["path"], "src/x.rs");
    assert_eq!(sent[0]["line"], 41);
    assert_eq!(sent[0]["side"], "RIGHT");
    assert_eq!(sent[0]["body"], "One line");
    assert!(
        sent[0].get("start_line").is_none() && sent[0].get("start_side").is_none(),
        "a comment on one line names no start"
    );
    assert_eq!(sent[1]["start_line"], 7);
    assert_eq!(sent[1]["start_side"], "RIGHT");
    assert_eq!(sent[1]["line"], 9);
}

#[tokio::test]
async fn a_request_for_changes_needs_a_summary_even_with_comments() {
    let (base, requests) = spawn(vec![]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    let comments = [draft("src/x.rs", None, 41, "One line")];
    assert!(matches!(
        github
            .submit_pull_request_review(
                "a/b",
                42,
                "abc123",
                "",
                PullReviewEvent::RequestChanges,
                &comments,
            )
            .await,
        Err(Error::Decode(message)) if message.contains("requires a body")
    ));
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_reply_posts_under_the_threads_root_and_decodes_the_comment() {
    let (base, requests) = spawn(vec![Response::new(
        "201 Created",
        review_comment(77, Some(31), "I will fix it"),
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let posted = github
        .reply_to_review_comment("a/b", 42, 31, "I will fix it")
        .await
        .unwrap();
    assert_eq!(posted.id, 77);
    assert_eq!(posted.in_reply_to_id, Some(31));
    assert_eq!(posted.node_id, "PRRC_77");
    assert_eq!(posted.html_url, "https://github.com/a/b/pull/42#discussion_r77");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/repos/a/b/pulls/42/comments");
    let json: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(json["in_reply_to"], 31);
    assert_eq!(json["body"], "I will fix it");
    assert!(json.get("commit_id").is_none());
}

#[tokio::test]
async fn a_comment_on_its_own_names_the_commit_it_was_written_against() {
    let (base, requests) = spawn(vec![Response::new(
        "201 Created",
        review_comment(78, None, "Older"),
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let posted = github
        .create_review_comment(
            "a/b",
            42,
            "0ld5ha",
            &draft("src/x.rs", Some(40), 41, "Older"),
        )
        .await
        .unwrap();
    assert_eq!(posted.id, 78);
    assert_eq!(posted.in_reply_to_id, None);

    {
        let requests = requests.lock().unwrap();
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/repos/a/b/pulls/42/comments");
        let json: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(json["commit_id"], "0ld5ha");
        assert_eq!(json["path"], "src/x.rs");
        assert_eq!(json["line"], 41);
        assert_eq!(json["start_line"], 40);
        assert_eq!(json["side"], "RIGHT");
        assert_eq!(json["start_side"], "RIGHT");
        assert_eq!(json["body"], "Older");
        assert!(json.get("in_reply_to").is_none());
    }

    // GitHub answers a comment it made with 201; anything else is a refusal.
    let (base, _) = spawn(vec![Response::new(
        "422 Unprocessable Entity",
        r#"{"message":"Line could not be resolved"}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    assert!(matches!(
        github
            .create_review_comment("a/b", 42, "0ld5ha", &draft("src/x.rs", None, 41, "x"))
            .await,
        Err(Error::Status { status: 422, body }) if body.contains("could not be resolved")
    ));
}
