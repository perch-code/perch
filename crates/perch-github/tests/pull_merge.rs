//! Pull-request merge state and mutations, and closing an issue, against
//! a scripted local server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::{
    CheckKind, CheckRunState, CheckState, Error, GitHub, IssueCloseReason, MergeMethod, MergeState, MergeableState,
    RepositoryPermission, ReviewDecision,
};

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

const OVERVIEW: &str = r#"{
  "data": {
    "repository": {
      "merge_commit_allowed": true,
      "squash_merge_allowed": true,
      "rebase_merge_allowed": false,
      "viewer_permission": "ADMIN",
      "pull_request": {
        "head_sha": "abc123",
        "mergeable": "MERGEABLE",
        "merge_state": "BLOCKED",
        "review_decision": "REVIEW_REQUIRED",
        "viewer_can_merge_as_admin": true,
        "status_check_rollup": {
          "state": "FAILURE",
          "contexts": { "nodes": [
            {
              "__typename": "CheckRun",
              "name": "check",
              "status": "COMPLETED",
              "conclusion": "SUCCESS",
              "details_url": "https://github.com/a/b/actions/runs/1/job/1",
              "started_at": "2026-09-20T08:00:00Z",
              "completed_at": "2026-09-20T08:02:00Z",
              "check_suite": { "workflow_run": {
                "event": "pull_request", "workflow": { "name": "PR Checks" }
              }}
            },
            {
              "__typename": "CheckRun",
              "name": "check",
              "status": "COMPLETED",
              "conclusion": "FAILURE",
              "details_url": "https://github.com/a/b/actions/runs/2/job/2",
              "started_at": "2026-09-20T09:00:00Z",
              "completed_at": "2026-09-20T09:04:00Z",
              "check_suite": { "workflow_run": {
                "event": "pull_request", "workflow": { "name": "PR Checks" }
              }}
            },
            {
              "__typename": "CheckRun",
              "name": "clippy",
              "status": "IN_PROGRESS",
              "conclusion": null,
              "details_url": "https://github.com/a/b/actions/runs/2/job/3",
              "started_at": "2026-09-20T09:00:00Z",
              "completed_at": null,
              "check_suite": { "workflow_run": {
                "event": "pull_request", "workflow": { "name": "PR Checks" }
              }}
            },
            {
              "__typename": "StatusContext",
              "context": "license/cla",
              "state": "SUCCESS",
              "description": "Contributor License Agreement is signed.",
              "target_url": "https://cla.example/status/1"
            },
            null
          ]}
        }
      }
    }
  }
}"#;

fn overview_page(
    head_sha: &str,
    state: &str,
    has_next_page: bool,
    end_cursor: Option<&str>,
    nodes: Vec<serde_json::Value>,
) -> String {
    serde_json::json!({
        "data": {
            "repository": {
                "merge_commit_allowed": true,
                "squash_merge_allowed": true,
                "rebase_merge_allowed": false,
                "viewer_permission": "ADMIN",
                "pull_request": {
                    "head_sha": head_sha,
                    "mergeable": "MERGEABLE",
                    "merge_state": "BLOCKED",
                    "review_decision": "REVIEW_REQUIRED",
                    "viewer_can_merge_as_admin": true,
                    "status_check_rollup": {
                        "state": state,
                        "contexts": {
                            "page_info": {
                                "has_next_page": has_next_page,
                                "end_cursor": end_cursor,
                            },
                            "nodes": nodes,
                        },
                    },
                },
            },
        },
    })
    .to_string()
}

fn check_run(
    name: &str,
    conclusion: &str,
    details_url: &str,
    started_at: &str,
) -> serde_json::Value {
    serde_json::json!({
        "__typename": "CheckRun",
        "name": name,
        "status": "COMPLETED",
        "conclusion": conclusion,
        "details_url": details_url,
        "started_at": started_at,
        "completed_at": started_at,
        "check_suite": { "workflow_run": {
            "event": "pull_request",
            "workflow": { "name": "PR Checks" },
        }},
    })
}

#[tokio::test]
async fn overview_combines_merge_permissions_and_deduplicated_status_rollup() {
    let (base, requests) = spawn(vec![Response::new("200 OK", OVERVIEW)]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let overview = github.fetch_pull_request_overview("a/b", 42).await.unwrap();

    assert_eq!(overview.head_sha, "abc123");
    assert_eq!(overview.mergeable, MergeableState::Mergeable);
    assert_eq!(overview.merge_state, MergeState::Blocked);
    assert_eq!(
        overview.review_decision,
        Some(ReviewDecision::ReviewRequested)
    );
    assert!(overview.viewer_can_merge_as_admin);
    assert_eq!(
        overview.viewer_permission,
        Some(RepositoryPermission::Admin)
    );
    assert!(overview.allowed_merge_methods.merge);
    assert!(overview.allowed_merge_methods.squash);
    assert!(!overview.allowed_merge_methods.rebase);
    assert_eq!(
        overview.checks.state,
        CheckState::Failing {
            workflow: "PR Checks".into()
        }
    );
    assert_eq!(overview.checks.runs.len(), 3, "the older retry is replaced");
    let check = overview
        .checks
        .runs
        .iter()
        .find(|check| check.name == "check")
        .unwrap();
    assert_eq!(check.state, CheckRunState::Failure);
    assert_eq!(check.kind, CheckKind::CheckRun);
    assert_eq!(check.event.as_deref(), Some("pull_request"));
    assert_eq!(
        check.url.as_deref(),
        Some("https://github.com/a/b/actions/runs/2/job/2")
    );
    let status = overview
        .checks
        .runs
        .iter()
        .find(|check| check.name == "license/cla")
        .unwrap();
    assert_eq!(status.kind, CheckKind::StatusContext);
    assert_eq!(status.state, CheckRunState::Success);

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/graphql");
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body["variables"]["owner"], "a");
    assert_eq!(body["variables"]["name"], "b");
    assert_eq!(body["variables"]["number"], 42);
    assert!(body["variables"]["contextsCursor"].is_null());
    let query = body["query"].as_str().unwrap();
    assert!(query.contains("statusCheckRollup"));
    assert!(query.contains("pageInfo"));
    assert!(query.contains("after: $contextsCursor"));
    assert!(query.contains("viewerCanMergeAsAdmin"));
}

#[tokio::test]
async fn overview_drains_context_pages_and_deduplicates_retries_across_them() {
    let first = overview_page(
        "abc123",
        "PENDING",
        true,
        Some("cursor-100"),
        vec![check_run(
            "check",
            "SUCCESS",
            "https://github.com/a/b/actions/runs/1/job/1",
            "2026-09-20T08:00:00Z",
        )],
    );
    let second = overview_page(
        "abc123",
        "FAILURE",
        false,
        None,
        vec![
            check_run(
                "check",
                "FAILURE",
                "https://github.com/a/b/actions/runs/2/job/2",
                "2026-09-20T09:00:00Z",
            ),
            serde_json::json!({
                "__typename": "StatusContext",
                "context": "license/cla",
                "state": "SUCCESS",
                "description": "Contributor License Agreement is signed.",
                "target_url": "https://cla.example/status/1",
            }),
        ],
    );
    let (base, requests) = spawn(vec![
        Response::new("200 OK", first),
        Response::new("200 OK", second),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let overview = github.fetch_pull_request_overview("a/b", 42).await.unwrap();

    assert_eq!(overview.head_sha, "abc123");
    assert_eq!(
        overview.checks.state,
        CheckState::Failing {
            workflow: "PR Checks".into()
        }
    );
    assert_eq!(overview.checks.runs.len(), 2);
    let check = overview
        .checks
        .runs
        .iter()
        .find(|check| check.name == "check")
        .unwrap();
    assert_eq!(check.state, CheckRunState::Failure);
    assert_eq!(
        check.url.as_deref(),
        Some("https://github.com/a/b/actions/runs/2/job/2")
    );
    assert_eq!(overview.checks.url.as_deref(), check.url.as_deref());

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let first: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    let second: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
    assert!(first["variables"]["contextsCursor"].is_null());
    assert_eq!(second["variables"]["contextsCursor"], "cursor-100");
}

#[tokio::test]
async fn overview_rejects_a_head_change_between_context_pages() {
    let first = overview_page(
        "abc123",
        "PENDING",
        true,
        Some("cursor-100"),
        vec![check_run(
            "check",
            "SUCCESS",
            "https://github.com/a/b/actions/runs/1/job/1",
            "2026-09-20T08:00:00Z",
        )],
    );
    let second = overview_page(
        "def456",
        "SUCCESS",
        false,
        None,
        vec![check_run(
            "clippy",
            "SUCCESS",
            "https://github.com/a/b/actions/runs/2/job/2",
            "2026-09-20T09:00:00Z",
        )],
    );
    let (base, requests) = spawn(vec![
        Response::new("200 OK", first),
        Response::new("200 OK", second),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    assert!(matches!(
        github.fetch_pull_request_overview("a/b", 42).await,
        Err(Error::Decode(message)) if message.contains("head changed")
            && message.contains("abc123") && message.contains("def456")
    ));
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn merge_methods_put_the_confirmed_head_sha_on_the_guarded_endpoint() {
    let responses = (0..3)
        .map(|_| {
            Response::new(
                "200 OK",
                r#"{"sha":"merged123","merged":true,"message":"Pull Request successfully merged"}"#,
            )
        })
        .collect();
    let (base, requests) = spawn(responses);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    for method in [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase] {
        let result = github
            .merge_pull_request("a/b", 42, "abc123", method)
            .await
            .unwrap();
        assert!(result.merged);
        assert_eq!(result.sha.as_deref(), Some("merged123"));
    }

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for (request, method) in requests.iter().zip(["merge", "squash", "rebase"]) {
        assert_eq!(request.method, "PUT");
        assert_eq!(request.path, "/repos/a/b/pulls/42/merge");
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["sha"], "abc123");
        assert_eq!(body["merge_method"], method);
    }
}

#[tokio::test]
async fn stale_head_conflicts_remain_distinct_github_errors() {
    let (base, _) = spawn(vec![Response::new(
        "409 Conflict",
        r#"{"message":"Head branch was modified"}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    assert!(matches!(
        github.merge_pull_request("a/b", 42, "old", MergeMethod::Squash).await,
        Err(Error::Status { status: 409, body }) if body.contains("Head branch was modified")
    ));
}

#[tokio::test]
async fn graphql_errors_are_not_mistaken_for_empty_overviews() {
    let (base, _) = spawn(vec![Response::new(
        "200 OK",
        r#"{"errors":[{"message":"Resource not accessible by personal access token"}]}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    assert!(matches!(
        github.fetch_pull_request_overview("a/b", 42).await,
        Err(Error::Graphql(message)) if message.contains("Resource not accessible")
    ));
}

#[tokio::test]
async fn closing_patches_the_pull_request_closed_and_reads_back_its_state() {
    let (base, requests) = spawn(vec![Response::new(
        "200 OK",
        r#"{"number":42,"state":"closed","merged":false,"closed_at":"2026-09-25T10:00:00Z",
            "head":{"sha":"abc123","ref":"feature","repo":{"full_name":"a/b"}},
            "base":{"sha":"base","ref":"main","repo":{"full_name":"a/b"}}}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let closed = github.close_pull_request("a/b", 42).await.unwrap();
    assert_eq!(closed.state, "closed");
    assert!(!closed.is_open());
    assert!(!closed.merged);

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "PATCH");
    assert_eq!(requests[0].path, "/repos/a/b/pulls/42");
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body, serde_json::json!({ "state": "closed" }));
}

#[tokio::test]
async fn closing_an_issue_patches_it_closed_with_the_reason() {
    let (base, requests) = spawn(vec![Response::new(
        "200 OK",
        r#"{"number":7,"title":"Fence","state":"closed","state_reason":"not_planned",
            "user":{"login":"rohan"},"updated_at":"2026-09-25T10:00:00Z"}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let closed = github
        .close_issue("a/b", 7, IssueCloseReason::NotPlanned)
        .await
        .unwrap();
    assert!(!closed.is_open());
    assert_eq!(closed.state_reason.as_deref(), Some("not_planned"));
    assert_eq!(closed.repo_full_name, "a/b");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "PATCH");
    assert_eq!(requests[0].path, "/repos/a/b/issues/7");
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({ "state": "closed", "state_reason": "not_planned" })
    );
}

#[tokio::test]
async fn whether_the_reader_may_close_an_issue_is_asked_of_graphql() {
    let (base, requests) = spawn(vec![
        Response::new(
            "200 OK",
            r#"{"data":{"repository":{"issue":{"viewer_can_close":true}}}}"#,
        ),
        Response::new("200 OK", r#"{"data":{"repository":{"issue":null}}}"#),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    assert!(github.viewer_can_close_issue("a/b", 7).await.unwrap());
    assert!(github.viewer_can_close_issue("a/b", 8).await.is_err());

    let requests = requests.lock().unwrap();
    assert_eq!(requests[0].path, "/graphql");
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert!(body["query"].as_str().unwrap().contains("viewerCanClose"));
    assert_eq!(
        body["variables"],
        serde_json::json!({ "owner": "a", "name": "b", "number": 7 })
    );
}
