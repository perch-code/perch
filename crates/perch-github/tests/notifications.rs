//! Notification polling and actions against a scripted local HTTP server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::{Error, GitHub};

const OLD_LAST_MODIFIED: &str = "Thu, 17 Sep 2026 20:03:33 GMT";
const NEW_LAST_MODIFIED: &str = "Sat, 19 Sep 2026 17:12:00 GMT";
const API_VERSION: &str = "2022-11-28";

fn thread_json(id: u32) -> String {
    format!(
        r#"{{
            "id": "{id}",
            "unread": true,
            "reason": "subscribed",
            "updated_at": "2026-09-17T18:00:00Z",
            "last_read_at": null,
            "subject": {{
                "title": "thread {id}",
                "type": "PullRequest",
                "url": "https://api.github.com/repos/a/b/pulls/{id}",
                "latest_comment_url": null
            }},
            "repository": {{
                "full_name": "a/b",
                "owner": {{ "avatar_url": "https://avatars/1" }}
            }}
        }}"#
    )
}

fn list_json(ids: impl IntoIterator<Item = u32>) -> String {
    let items: Vec<String> = ids.into_iter().map(thread_json).collect();
    format!("[{}]", items.join(","))
}

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }

    fn header_values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }
}

struct Response {
    status: &'static str,
    headers: Vec<(String, String)>,
    body: String,
}

impl Response {
    fn new(status: &'static str, body: impl Into<String>) -> Self {
        Self { status, headers: Vec::new(), body: body.into() }
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let mut start = start.split_whitespace();
    let method = start.next()?.to_string();
    let path = start.next()?.to_string();

    let mut headers = Vec::new();
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
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn write_response(stream: &mut TcpStream, response: Response) {
    let extra_headers = response
        .headers
        .into_iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let wire = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {length}\r\n\
         {extra_headers}\
         Connection: close\r\n\
         \r\n{body}",
        status = response.status,
        length = response.body.len(),
        body = response.body,
    );
    stream.write_all(wire.as_bytes()).expect("write response");
    stream.flush().expect("flush response");
}

/// Serve exactly the scripted responses. An unexpected extra request sees a
/// closed listener and fails the test through the client call.
fn spawn(responses: Vec<Response>) -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();

    std::thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept request");
            let request = read_request(&mut stream).expect("read request");
            seen.lock().unwrap().push(request);
            write_response(&mut stream, response);
        }
    });

    (format!("http://{addr}"), requests)
}

#[tokio::test]
async fn an_unconditional_poll_fetches_every_page_and_pins_the_api_version() {
    let responses = vec![
        Response::new("200 OK", list_json(1..=50))
            .header("Last-Modified", NEW_LAST_MODIFIED)
            .header("X-Poll-Interval", "75"),
        Response::new("200 OK", list_json(51..=52)).header("X-Poll-Interval", "120"),
    ];
    let (base, requests) = spawn(responses);
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let result = gh.list_threads().await.expect("list threads");

    assert!(!result.unchanged);
    assert_eq!(result.threads.len(), 52);
    assert_eq!(result.threads[0].id, "1");
    assert_eq!(result.threads[51].id, "52");
    assert_eq!(result.last_modified.as_deref(), Some(NEW_LAST_MODIFIED));
    assert_eq!(result.poll_interval, std::time::Duration::from_secs(120));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].path,
        "/notifications?all=true&participating=false&per_page=50&page=1"
    );
    assert_eq!(
        requests[1].path,
        "/notifications?all=true&participating=false&per_page=50&page=2"
    );
    for request in requests.iter() {
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.header_values("x-github-api-version"),
            vec![API_VERSION]
        );
        assert_eq!(request.header("if-modified-since"), None);
    }
}

#[tokio::test]
async fn a_304_probe_keeps_the_cached_list_and_validator() {
    let responses = vec![Response::new("304 Not Modified", "").header("X-Poll-Interval", "81")];
    let (base, requests) = spawn(responses);
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let result = gh
        .list_threads_since(Some(OLD_LAST_MODIFIED))
        .await
        .expect("list threads");

    assert!(result.unchanged);
    assert!(result.threads.is_empty());
    assert_eq!(result.last_modified.as_deref(), Some(OLD_LAST_MODIFIED));
    assert_eq!(result.poll_interval, std::time::Duration::from_secs(81));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("if-modified-since"), Some(OLD_LAST_MODIFIED));
}

#[tokio::test]
async fn a_changed_conditional_response_is_the_complete_first_page() {
    let responses = vec![
        Response::new("200 OK", list_json(1..=3))
            .header("Last-Modified", NEW_LAST_MODIFIED)
            .header("X-Poll-Interval", "90"),
    ];
    let (base, requests) = spawn(responses);
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let result = gh
        .list_threads_since(Some(OLD_LAST_MODIFIED))
        .await
        .expect("list threads");

    assert!(!result.unchanged);
    assert_eq!(
        result.threads.iter().map(|thread| thread.id.as_str()).collect::<Vec<_>>(),
        vec!["1", "2", "3"],
        "the changed representation replaces the cached inbox"
    );
    assert_eq!(result.last_modified.as_deref(), Some(NEW_LAST_MODIFIED));
    assert_eq!(
        result.poll_interval,
        std::time::Duration::from_secs(90),
        "the response controls the next allowed poll"
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("if-modified-since"), Some(OLD_LAST_MODIFIED));
}

#[tokio::test]
async fn notification_actions_use_githubs_exact_methods_paths_and_body() {
    let responses = vec![
        Response::new("205 Reset Content", ""),
        Response::new("204 No Content", ""),
        Response::new("200 OK", r#"{"subscribed":false,"ignored":true}"#),
    ];
    let (base, requests) = spawn(responses);
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    gh.mark_thread_read("41").await.expect("mark read");
    gh.mark_thread_done("42").await.expect("mark done");
    gh.ignore_thread("43").await.expect("ignore thread");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        (requests[0].method.as_str(), requests[0].path.as_str()),
        ("PATCH", "/notifications/threads/41")
    );
    assert_eq!(
        (requests[1].method.as_str(), requests[1].path.as_str()),
        ("DELETE", "/notifications/threads/42")
    );
    assert_eq!(
        (requests[2].method.as_str(), requests[2].path.as_str()),
        ("PUT", "/notifications/threads/43/subscription")
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&requests[2].body).unwrap(),
        serde_json::json!({ "ignored": true })
    );
    for request in requests.iter() {
        assert_eq!(
            request.header_values("x-github-api-version"),
            vec![API_VERSION]
        );
    }
}

#[tokio::test]
async fn idempotent_304_action_responses_are_successful() {
    let responses = vec![
        Response::new("304 Not Modified", ""),
        Response::new("304 Not Modified", ""),
    ];
    let (base, _) = spawn(responses);
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    gh.mark_thread_read("41").await.expect("already read");
    gh.ignore_thread("41").await.expect("already ignored");
}

#[tokio::test]
async fn action_failures_keep_rate_limit_classification() {
    let response = Response::new("403 Forbidden", r#"{"message":"API rate limit exceeded"}"#)
        .header("X-RateLimit-Remaining", "0")
        .header("X-RateLimit-Reset", "1790000000")
        .header("Retry-After", "3600");
    let (base, _) = spawn(vec![response]);
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    assert!(matches!(
        gh.mark_thread_read("41").await,
        Err(Error::RateLimited {
            reset_epoch: Some(1790000000),
            retry_after_secs: Some(3600)
        })
    ));
}

#[tokio::test]
async fn a_successful_last_request_gates_the_next_call_without_sending_it() {
    let reset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3_600;
    let response = Response::new("205 Reset Content", "")
        .header("X-RateLimit-Remaining", "0")
        .header("X-RateLimit-Reset", &reset.to_string());
    let (base, requests) = spawn(vec![response]);
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    gh.mark_thread_read("41").await.expect("the final allowed request succeeds");
    assert!(matches!(
        gh.mark_thread_done("42").await,
        Err(Error::RateLimited {
            reset_epoch: Some(deadline),
            retry_after_secs: None
        }) if deadline == reset as i64
    ));
    assert_eq!(requests.lock().unwrap().len(), 1, "the gated call never reached the server");
}

#[tokio::test]
async fn a_non_numeric_thread_id_is_rejected_before_a_request() {
    let gh = GitHub::new("token").expect("client");
    assert!(matches!(
        gh.mark_thread_done("41/../../notifications").await,
        Err(Error::Decode(message)) if message.contains("not numeric")
    ));
}
