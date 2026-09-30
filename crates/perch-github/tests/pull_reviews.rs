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

#[tokio::test]
async fn an_issue_comment_posts_to_the_conversation_and_decodes_the_comment() {
    let (base, requests) = spawn(vec![Response::new(
        "201 Created",
        serde_json::json!({
            "id": 910,
            "node_id": "IC_910",
            "user": { "login": "reader", "avatar_url": "https://avatars/reader" },
            "body": "Looks good from here.",
            "created_at": "2026-09-21T18:00:00Z",
            "updated_at": "2026-09-21T18:00:00Z",
            "html_url": "https://github.com/a/b/pull/42#issuecomment-910",
        })
        .to_string(),
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let posted = github
        .create_issue_comment("a/b", 42, "Looks good from here.")
        .await
        .unwrap();
    assert_eq!(posted.id, 910);
    assert_eq!(posted.node_id, "IC_910");
    assert_eq!(posted.user, "reader");
    assert_eq!(posted.user_avatar_url, "https://avatars/reader");
    assert_eq!(posted.body, "Looks good from here.");
    assert_eq!(
        posted.html_url,
        "https://github.com/a/b/pull/42#issuecomment-910"
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/repos/a/b/issues/42/comments");
    let json: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(json, serde_json::json!({ "body": "Looks good from here." }));
}

#[tokio::test]
async fn an_issue_opens_with_its_title_and_body_and_decodes_the_issue() {
    let (base, requests) = spawn(vec![Response::new(
        "201 Created",
        serde_json::json!({
            "number": 58,
            "node_id": "I_58",
            "title": "The manifest races",
            "body": "Two writers.",
            "state": "open",
            "user": { "login": "reader", "avatar_url": "https://avatars/reader" },
            "comments": 0,
            "created_at": "2026-09-27T10:00:00Z",
            "updated_at": "2026-09-27T10:00:00Z",
        })
        .to_string(),
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let issue = github
        .create_issue("a/b", "The manifest races", "Two writers.")
        .await
        .unwrap();
    assert_eq!(issue.number, 58);
    assert_eq!(issue.title, "The manifest races");
    assert_eq!(issue.repo_full_name, "a/b");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/repos/a/b/issues");
    let json: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "title": "The manifest races", "body": "Two writers." })
    );
}

#[tokio::test]
async fn an_issue_without_a_title_is_refused_before_the_request() {
    let (base, requests) = spawn(vec![]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    assert!(matches!(
        github.create_issue("a/b", "  ", "body").await,
        Err(Error::Decode(_))
    ));
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_empty_issue_comment_is_refused_before_the_request() {
    let (base, requests) = spawn(vec![]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    assert!(matches!(
        github.create_issue_comment("a/b", 42, "  \n").await,
        Err(Error::Decode(_))
    ));
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn resolving_a_thread_names_it_and_reads_back_whether_it_is_resolved() {
    let (base, requests) = spawn(vec![
        Response::new(
            "200 OK",
            r#"{"data":{"resolveReviewThread":{"thread":{"isResolved":true}}}}"#,
        ),
        Response::new(
            "200 OK",
            r#"{"data":{"unresolveReviewThread":{"thread":{"isResolved":false}}}}"#,
        ),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    assert!(github.set_review_thread_resolved("PRRT_1", true).await.unwrap());
    assert!(!github.set_review_thread_resolved("PRRT_1", false).await.unwrap());

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!((requests[0].method.as_str(), requests[0].path.as_str()), ("POST", "/graphql"));
    let asked = |i: usize| -> serde_json::Value { serde_json::from_str(&requests[i].body).unwrap() };
    let resolve = asked(0);
    let query = resolve["query"].as_str().unwrap();
    assert!(query.starts_with("mutation("), "{query}");
    assert!(query.contains("resolveReviewThread(input: {threadId: $thread})"), "{query}");
    assert!(!query.contains("unresolve"), "{query}");
    assert_eq!(resolve["variables"]["thread"], "PRRT_1");
    let unresolve = asked(1);
    assert!(unresolve["query"].as_str().unwrap().contains("unresolveReviewThread("));
    assert_eq!(unresolve["variables"]["thread"], "PRRT_1");
}

#[tokio::test]
async fn a_thread_github_will_not_resolve_is_an_error_and_no_id_asks_nothing() {
    let (base, requests) = spawn(vec![
        Response::new(
            "200 OK",
            r#"{"data":{"resolveReviewThread":null},"errors":[{"message":"Resource not accessible by personal access token"}]}"#,
        ),
        Response::new("200 OK", r#"{"data":{"resolveReviewThread":null}}"#),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let refused = github.set_review_thread_resolved("PRRT_1", true).await.unwrap_err();
    assert!(
        matches!(&refused, Error::Graphql(message) if message.contains("not accessible")),
        "{refused:?}"
    );
    let empty = github.set_review_thread_resolved("PRRT_1", true).await.unwrap_err();
    assert!(matches!(empty, Error::Decode(_)), "{empty:?}");
    assert_eq!(requests.lock().unwrap().len(), 2);

    let none = github.set_review_thread_resolved("", false).await.unwrap_err();
    assert!(matches!(none, Error::Decode(_)), "{none:?}");
    assert_eq!(requests.lock().unwrap().len(), 2, "no id, no request");
}

#[tokio::test]
async fn a_file_is_read_at_a_commit_and_a_change_to_it_committed_over_the_head() {
    let (base, requests) = spawn(vec![
        Response::new(
            "200 OK",
            r#"{"data":{"repository":{"object":{"text":"a\nb\n","isBinary":false,"isTruncated":false}}}}"#,
        ),
        Response::new(
            "200 OK",
            r#"{"data":{"repository":{"object":{"text":null,"isBinary":true,"isTruncated":false}}}}"#,
        ),
        Response::new("200 OK", r#"{"data":{"repository":{"object":null}}}"#),
        Response::new(
            "200 OK",
            r#"{"data":{"createCommitOnBranch":{"commit":{"oid":"c2"}}}}"#,
        ),
        Response::new(
            "200 OK",
            r#"{"data":{"createCommitOnBranch":null},"errors":[{"message":"Expected branch to point to \"h1\" but it did not"}]}"#,
        ),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    assert_eq!(github.file_text("a/b", "h1", "src/x.rs").await.unwrap(), Ok("a\nb\n".into()));
    assert_eq!(
        github.file_text("a/b", "h1", "logo.png").await.unwrap(),
        Err(perch_github::Unreadable::Binary)
    );
    assert_eq!(
        github.file_text("a/b", "h1", "gone.rs").await.unwrap(),
        Err(perch_github::Unreadable::Missing)
    );
    let oid = github
        .commit_file("fork/b", "feature", "h1", "src/x.rs", "a\nB\n", "Update src/x.rs", "")
        .await
        .unwrap();
    assert_eq!(oid, "c2");
    let moved = github
        .commit_file("fork/b", "feature", "h1", "src/x.rs", "a\nB\n", "Update src/x.rs", "why")
        .await
        .unwrap_err();
    assert!(matches!(&moved, Error::Graphql(m) if m.contains("Expected branch")), "{moved:?}");
    assert!(
        github.commit_file("fork/b", "feature", "h1", "src/x.rs", "", " ", "").await.is_err(),
        "no message, no request"
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    let asked = |i: usize| -> serde_json::Value { serde_json::from_str(&requests[i].body).unwrap() };
    let read = asked(0);
    assert_eq!(read["variables"]["owner"], "a");
    assert_eq!(read["variables"]["name"], "b");
    assert_eq!(read["variables"]["expression"], "h1:src/x.rs");
    let commit = asked(3);
    assert!(commit["query"].as_str().unwrap().contains("createCommitOnBranch(input: $input)"));
    let input = &commit["variables"]["input"];
    assert_eq!(input["branch"]["repositoryNameWithOwner"], "fork/b");
    assert_eq!(input["branch"]["branchName"], "feature");
    assert_eq!(input["expectedHeadOid"], "h1");
    assert_eq!(input["message"]["headline"], "Update src/x.rs");
    assert!(input["message"].get("body").is_none(), "no body when there is none");
    assert_eq!(input["fileChanges"]["additions"][0]["path"], "src/x.rs");
    assert_eq!(input["fileChanges"]["additions"][0]["contents"], "YQpCCg==");
    assert_eq!(asked(4)["variables"]["input"]["message"]["body"], "why");
}
