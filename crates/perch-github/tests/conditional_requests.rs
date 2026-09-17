//! Regression tests for how the notifications endpoint answers conditional
//! requests.
//!
//! GitHub treats `If-Modified-Since` on `/notifications` as a *filter*, not a
//! cache validator: a 200 carries only the threads updated since that
//! instant. Acting on that partial list drops every thread that did not
//! change, which once truncated the local cache from 31 threads to 1.
//!
//! These tests stand up a local server that mimics the real behaviour, so the
//! client's handling is pinned without touching the network.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use perch_github::GitHub;

const LAST_MODIFIED: &str = "Thu, 17 Sep 2026 20:03:33 GMT";

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

fn list_json(n: u32) -> String {
    let items: Vec<String> = (1..=n).map(thread_json).collect();
    format!("[{}]", items.join(","))
}

struct Request {
    path: String,
    if_modified_since: Option<String>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    if start.trim().is_empty() {
        return None;
    }
    let path = start.split_whitespace().nth(1)?.to_string();

    let mut if_modified_since = None;
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
            if name == "if-modified-since" {
                if_modified_since = Some(value);
            } else if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
        }
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).ok()?;
    }

    Some(Request { path, if_modified_since })
}

fn write_response(stream: &mut TcpStream, status: &str, extra: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         X-Poll-Interval: 60\r\n\
         {extra}\
         Connection: close\r\n\
         \r\n{body}",
        len = body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// A server that answers like GitHub does: a conditional request gets the
/// filtered subset, an unconditional one gets the whole inbox.
fn spawn_github_like(full: u32, partial: u32) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let conditional = Arc::new(AtomicUsize::new(0));
    let unconditional = Arc::new(AtomicUsize::new(0));

    let c = conditional.clone();
    let u = unconditional.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(req) = read_request(&mut stream) else { continue };

            // Page 2 is always empty, which ends pagination.
            if req.path.contains("page=2") {
                write_response(&mut stream, "200 OK", "", "[]");
                continue;
            }

            if req.if_modified_since.is_some() {
                c.fetch_add(1, Ordering::SeqCst);
                write_response(
                    &mut stream,
                    "200 OK",
                    &format!("Last-Modified: {LAST_MODIFIED}\r\n"),
                    &list_json(partial),
                );
            } else {
                u.fetch_add(1, Ordering::SeqCst);
                write_response(
                    &mut stream,
                    "200 OK",
                    &format!("Last-Modified: {LAST_MODIFIED}\r\n"),
                    &list_json(full),
                );
            }
        }
    });

    (format!("http://{addr}"), conditional, unconditional)
}

/// A server that reports nothing has changed.
fn spawn_not_modified() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests = Arc::new(AtomicUsize::new(0));

    let r = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(req) = read_request(&mut stream) else { continue };
            r.fetch_add(1, Ordering::SeqCst);
            if req.if_modified_since.is_some() {
                write_response(&mut stream, "304 Not Modified", "", "");
            } else {
                write_response(&mut stream, "200 OK", "", &list_json(3));
            }
        }
    });

    (format!("http://{addr}"), requests)
}

#[tokio::test]
async fn a_changed_inbox_is_read_in_full_not_from_the_filtered_response() {
    // The server offers 1 thread conditionally and 5 unconditionally, which
    // is exactly the shape that truncated the cache.
    let (base, conditional, unconditional) = spawn_github_like(5, 1);
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let result = gh
        .list_threads(Some(LAST_MODIFIED))
        .await
        .expect("list threads");

    let threads = result.threads.expect("a changed inbox returns a list");
    assert_eq!(
        threads.len(),
        5,
        "the partial conditional response must not become the inbox"
    );
    assert_eq!(conditional.load(Ordering::SeqCst), 1, "one cheap probe");
    assert!(
        unconditional.load(Ordering::SeqCst) >= 1,
        "a change must trigger a full, unconditional read"
    );
}

#[tokio::test]
async fn an_unchanged_inbox_costs_one_request_and_returns_no_list() {
    let (base, requests) = spawn_not_modified();
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let result = gh
        .list_threads(Some(LAST_MODIFIED))
        .await
        .expect("list threads");

    assert!(
        result.threads.is_none(),
        "an unchanged inbox must leave the cached list alone"
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "a 304 must not be followed by a full read"
    );
    assert_eq!(result.poll_interval, std::time::Duration::from_secs(60));
}

#[tokio::test]
async fn a_first_run_with_no_timestamp_reads_the_full_inbox() {
    let (base, conditional, unconditional) = spawn_github_like(5, 1);
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let result = gh.list_threads(None).await.expect("list threads");

    assert_eq!(result.threads.expect("list").len(), 5);
    assert_eq!(
        conditional.load(Ordering::SeqCst),
        0,
        "no timestamp means no probe"
    );
    assert_eq!(unconditional.load(Ordering::SeqCst), 1);
    assert_eq!(result.last_modified.as_deref(), Some(LAST_MODIFIED));
}
