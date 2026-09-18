//! The discussion fetch against a local server, to pin pagination.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use perch_github::{GitHub, PullEvent, PullEventKind, Side};

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

fn respond(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn comment(id: usize) -> String {
    format!(
        r#"{{"id":{id},"user":{{"login":"u{id}"}},"body":"comment {id}","created_at":"2026-09-15T09:{:02}:00Z","html_url":"https://github.com/a/b/pull/1#issuecomment-{id}"}}"#,
        id % 60
    )
}

fn page(items: Vec<String>) -> String {
    format!("[{}]", items.join(","))
}

/// A pull request with 150 conversation comments, one review, and two line
/// comments, served a hundred a page.
fn spawn(comments: usize) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(path) = read_path(&mut stream) else { continue };
            counter.fetch_add(1, Ordering::SeqCst);
            let page_no: usize = path
                .split("&page=")
                .nth(1)
                .and_then(|p| p.split('&').next())
                .and_then(|p| p.parse().ok())
                .unwrap_or(1);
            let body = if path.contains("/issues/1/comments") {
                let start = (page_no - 1) * 100;
                let end = (start + 100).min(comments);
                page((start..end).map(|i| comment(i + 1)).collect())
            } else if path.contains("/pulls/1/reviews") {
                page(vec![r#"{"id":9,"user":{"login":"r"},"body":"lgtm","state":"APPROVED","submitted_at":"2026-09-16T11:40:00Z","html_url":"https://github.com/a/b/pull/1#pullrequestreview-9"}"#.into()])
            } else if path.contains("/pulls/1/comments") {
                page(vec![
                    r#"{"id":3,"pull_request_review_id":9,"user":{"login":"r"},"body":"here","path":"src/x.rs","line":41,"side":"LEFT","diff_hunk":"@@ -40,3 +40,3 @@","created_at":"2026-09-16T11:38:00Z","html_url":"https://github.com/a/b/pull/1#discussion_r3"}"#.into(),
                    r#"{"id":4,"pull_request_review_id":9,"in_reply_to_id":3,"user":{"login":"a"},"body":"ok","path":"src/x.rs","line":41,"created_at":"2026-09-16T11:39:00Z","html_url":"https://github.com/a/b/pull/1#discussion_r4"}"#.into(),
                ])
            } else if path.contains("/pulls/1") {
                r#"{"number":1,"body":"The description.","created_at":"2026-09-14T14:02:00Z",
                    "merged":true,"merged_at":"2026-09-17T15:00:00Z","merged_by":{"login":"r"},
                    "closed_at":"2026-09-17T15:00:00Z","head":{"sha":"h"},"base":{"sha":"b"}}"#.into()
            } else {
                "[]".into()
            };
            respond(&mut stream, &body);
        }
    });
    (format!("http://{addr}"), requests)
}

#[tokio::test]
async fn every_page_of_the_conversation_is_read() {
    let (base, requests) = spawn(150);
    let gh = GitHub::with_base_uri("token", &base).unwrap();

    let d = gh.fetch_discussion("a/b", 1).await.unwrap();

    assert_eq!(d.body, "The description.");
    assert_eq!(d.opened_at, Some("2026-09-14T14:02:00Z".parse().unwrap()));
    assert_eq!(d.comments.len(), 150, "two pages, the second short");
    assert_eq!(d.comments[149].body, "comment 150");
    assert_eq!(d.reviews.len(), 1);
    assert_eq!(d.reviews[0].state, "APPROVED");
    assert_eq!(d.review_comments.len(), 2);
    assert_eq!(d.review_comments[1].in_reply_to_id, Some(3));
    assert_eq!(d.review_comments[0].location(), "src/x.rs:41");
    assert_eq!(d.review_comments[0].side, Side::Left);
    assert_eq!(d.review_comments[0].diff_hunk, "@@ -40,3 +40,3 @@");
    assert_eq!(d.review_comments[1].side, Side::Right, "no side is the head side");
    // Opened and merged, out of the pull request's own payload.
    assert_eq!(
        d.events,
        vec![
            PullEvent { kind: PullEventKind::Opened, at: "2026-09-14T14:02:00Z".parse().unwrap() },
            PullEvent {
                kind: PullEventKind::Merged { by: Some("r".into()) },
                at: "2026-09-17T15:00:00Z".parse().unwrap(),
            },
        ]
    );
    // The pull request, two comment pages, one reviews page, one line page.
    assert_eq!(requests.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn an_exact_page_costs_one_more_empty_request_and_nothing_else() {
    let (base, _) = spawn(100);
    let gh = GitHub::with_base_uri("token", &base).unwrap();
    let d = gh.fetch_discussion("a/b", 1).await.unwrap();
    assert_eq!(d.comments.len(), 100);
}
