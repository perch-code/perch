//! Pull-request review mutations against a scripted local server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::{Error, GitHub, PullReviewEvent};

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
            .submit_pull_request_review("a/b", 42, "abc123", body, event)
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
            .submit_pull_request_review("a/b", 42, "abc123", "  ", PullReviewEvent::Comment)
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
            )
            .await,
        Err(Error::Decode(message)) if message.contains("requires a body")
    ));
    assert!(matches!(
        github
            .submit_pull_request_review("a/b", 42, " bad ", "", PullReviewEvent::Approve)
            .await,
        Err(Error::Decode(message)) if message.contains("commit SHA")
    ));

    github
        .submit_pull_request_review("a/b", 42, "abc123", "", PullReviewEvent::Approve)
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
            .submit_pull_request_review("a/b", 42, "abc123", "", PullReviewEvent::Approve)
            .await,
        Err(Error::Status { status: 422, body })
            if body.contains("Can not approve your own pull request")
    ));
}
