//! Issue search against a local server: the query as sent, the page as
//! read, and the search API's own rate-limit window.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

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

fn respond(stream: &mut TcpStream, status: &str, headers: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

const PAGE: &str = r#"{"total_count": 1, "items": [
    {"number": 7, "title": "Fence", "state": "open",
     "user": {"login": "rohan", "avatar_url": ""},
     "updated_at": "2026-09-20T10:00:00Z", "html_url": "https://github.com/a/b/pull/7",
     "repository_url": "https://api.github.com/repos/a/b", "pull_request": {"merged_at": null}}
]}"#;

const ISSUE: &str =
    r#"{"number": 42, "title": "t", "state": "open", "updated_at": "2026-09-16T11:40:00Z"}"#;

/// Searches answer with `PAGE` until `limited`, then with the search
/// limit's 403; the issues endpoint always answers.
fn spawn(limited: usize) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let seen = paths.clone();
    std::thread::spawn(move || {
        let mut searches = 0;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(path) = read_path(&mut stream) else {
                continue;
            };
            seen.lock().unwrap().push(path.clone());
            if path.starts_with("/search/issues") {
                searches += 1;
                if searches > limited {
                    respond(
                        &mut stream,
                        "403 Forbidden",
                        "X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 4102444800\r\nX-RateLimit-Resource: search\r\n",
                        r#"{"message":"API rate limit exceeded"}"#,
                    );
                } else {
                    respond(&mut stream, "200 OK", "X-RateLimit-Remaining: 29\r\n", PAGE);
                }
            } else {
                respond(&mut stream, "200 OK", "", ISSUE);
            }
        }
    });
    (format!("http://{addr}"), paths)
}

#[tokio::test]
async fn a_search_sends_its_query_and_reads_the_page() {
    let (base, paths) = spawn(usize::MAX);
    let gh = GitHub::with_base_uri("token", &base).unwrap();

    let page = gh
        .search_issues("is:pr review-requested:@me is:open", 2, 50)
        .await
        .unwrap();

    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].kind, ItemKind::PullRequest);
    assert_eq!(page.hits[0].repo_full_name, "a/b");
    assert!(!page.more);
    let path = paths.lock().unwrap()[0].clone();
    assert_eq!(
        path,
        "/search/issues?q=is%3Apr%20review%2Drequested%3A%40me%20is%3Aopen&sort=updated&order=desc&per_page=50&page=2"
    );
}

#[tokio::test]
async fn a_spent_search_window_waits_and_holds_up_nothing_else() {
    let (base, paths) = spawn(0);
    let gh = GitHub::with_base_uri("token", &base).unwrap();

    assert!(matches!(
        gh.search_issues("author:@me", 1, 50).await.unwrap_err(),
        Error::RateLimited { .. }
    ));
    assert!(matches!(
        gh.search_issues("author:@me", 1, 50).await.unwrap_err(),
        Error::RateLimited { .. }
    ));
    assert_eq!(
        paths.lock().unwrap().len(),
        1,
        "the second search waited for the reset"
    );

    gh.fetch_issue("a/b", 42).await.unwrap();
    assert_eq!(
        paths.lock().unwrap().len(),
        2,
        "the rest of the API goes on"
    );
}
