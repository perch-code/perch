//! The notifications poll against a local server, to pin its shape.
//!
//! One unconditional request for the first page. GitHub treats
//! `If-Modified-Since` on `/notifications` as a filter rather than a cache
//! validator, so a conditional request would come back with only the
//! threads that moved, and nothing here keeps a list to fill the rest in
//! from. The server here reads the headers it was sent, so the test can say
//! the client asked plainly.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::GitHub;

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

/// One request as the server saw it: the path, and the request headers by
/// lower-cased name.
struct Request {
    path: String,
    headers: Vec<(String, String)>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    if start.trim().is_empty() {
        return None;
    }
    let path = start.split_whitespace().nth(1)?.to_string();

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
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).ok()?;
    }

    Some(Request { path, headers })
}

fn write_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         X-Poll-Interval: 75\r\n\
         Last-Modified: Thu, 17 Sep 2026 20:03:33 GMT\r\n\
         Connection: close\r\n\
         \r\n{body}",
        len = body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// A server that answers every request with the same page and writes down
/// what it was asked.
fn spawn(body: String) -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests = Arc::new(Mutex::new(Vec::new()));

    let seen = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(req) = read_request(&mut stream) else { continue };
            seen.lock().unwrap().push(req);
            write_response(&mut stream, &body);
        }
    });

    (format!("http://{addr}"), requests)
}

#[tokio::test]
async fn a_poll_is_one_plain_request_for_the_first_page() {
    let (base, requests) = spawn(list_json(5));
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let result = gh.list_threads().await.expect("list threads");

    assert_eq!(result.threads.len(), 5);
    assert_eq!(result.threads[0].id, "1");
    assert_eq!(result.threads[0].repository.full_name, "a/b");
    assert_eq!(
        result.poll_interval,
        std::time::Duration::from_secs(75),
        "the server's pace is what the poll runs at"
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "one request, whatever the server said last time");
    assert_eq!(
        requests[0].path,
        "/notifications?all=true&participating=false&per_page=50",
        "the first page, read and unread alike"
    );
    assert!(
        !requests[0].headers.iter().any(|(name, _)| name == "if-modified-since"),
        "asked plainly: a conditional answer would be a filtered list"
    );
}
