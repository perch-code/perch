//! A long timeline is read for its first page and its newest, the ends a
//! reader looks at, against a local server that pages it as GitHub does.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use perch_github::{GitHub, PullEventKind};

/// The page each request asked for, and whether it was asked with a tag.
#[derive(Debug)]
struct Request {
    page: usize,
    tagged: bool,
}

fn read_request(stream: &mut TcpStream) -> Option<(String, Option<String>)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let path = start.split_whitespace().nth(1)?.to_string();
    let mut if_none_match = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim_end().is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            if name.eq_ignore_ascii_case("if-none-match") {
                if_none_match = Some(value.trim().to_string());
            }
        }
    }
    Some((path, if_none_match))
}

/// Event `i` of the timeline: a label a minute, named for its place.
fn event(i: usize) -> String {
    format!(
        r#"{{"event":"labeled","actor":{{"login":"r"}},"created_at":"2026-09-{:02}T{:02}:{:02}:00Z","label":{{"name":"l{i}","color":"d73a4a"}}}}"#,
        1 + i / 1440,
        (i / 60) % 24,
        i % 60,
    )
}

/// Serve issue 7's timeline, `total` events long, oldest first, a hundred
/// to a page. Each page's tag says which events it holds, so a page that
/// still holds the same ones is a 304. A 200 carries the `Link` header
/// GitHub sends; a 304 carries none, which GitHub need not send.
fn spawn(total: Arc<AtomicUsize>) -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some((path, if_none_match)) = read_request(&mut stream) else {
                continue;
            };
            assert!(
                path.starts_with("/repos/a/b/issues/7/timeline?per_page=100&page="),
                "{path}"
            );
            let page: usize = path.rsplit("page=").next().unwrap().parse().unwrap();
            seen.lock().unwrap().push(Request {
                page,
                tagged: if_none_match.is_some(),
            });
            let total = total.load(Ordering::SeqCst);
            let events: Vec<String> = ((page - 1) * 100..(page * 100).min(total))
                .map(event)
                .collect();
            let etag = format!(r#""{page}-{}""#, events.len());
            let wire = if if_none_match.as_deref() == Some(etag.as_str()) {
                format!("HTTP/1.1 304 Not Modified\r\nETag: {etag}\r\nConnection: close\r\n\r\n")
            } else {
                let last = total.div_ceil(100);
                let at = |page: usize| {
                    format!("<https://api.github.com/repositories/1/issues/7/timeline?per_page=100&page={page}>")
                };
                let link = if last <= 1 {
                    String::new()
                } else if page < last {
                    format!(
                        "Link: {}; rel=\"next\", {}; rel=\"last\"\r\n",
                        at(page + 1),
                        at(last)
                    )
                } else {
                    format!(
                        "Link: {}; rel=\"prev\", {}; rel=\"first\"\r\n",
                        at(page - 1),
                        at(1)
                    )
                };
                let body = format!("[{}]", events.join(","));
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nETag: {etag}\r\n{link}Connection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            let _ = stream.write_all(wire.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{address}"), requests)
}

/// The labels the timeline came back with, by name.
async fn labels(github: &GitHub) -> Vec<String> {
    github
        .fetch_timeline("a/b", 7, &[])
        .await
        .unwrap()
        .into_iter()
        .map(|e| match e.kind {
            PullEventKind::Labeled { label } => label.name,
            other => panic!("{other:?}"),
        })
        .collect()
}

/// The pages asked for since the last call, in order.
fn pages(requests: &Mutex<Vec<Request>>) -> Vec<usize> {
    requests.lock().unwrap().drain(..).map(|r| r.page).collect()
}

#[tokio::test]
async fn a_long_timeline_keeps_its_first_page_and_its_newest() {
    let (base, requests) = spawn(Arc::new(AtomicUsize::new(2450)));
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let labels = labels(&github).await;

    // Twenty-five pages: the first, then the nine newest, the last half
    // full. Ten pages read, as before, but the latest events are in them.
    assert_eq!(pages(&requests), [1, 17, 18, 19, 20, 21, 22, 23, 24, 25]);
    assert_eq!(labels.len(), 100 + 800 + 50);
    assert_eq!(labels.first().map(String::as_str), Some("l0"));
    assert_eq!(labels.last().map(String::as_str), Some("l2449"));
    assert!(labels.contains(&"l99".to_string()));
    assert!(labels.contains(&"l1600".to_string()));
    assert!(!labels.contains(&"l100".to_string()));
    assert!(!labels.contains(&"l1599".to_string()));
}

#[tokio::test]
async fn a_timeline_of_ten_pages_is_read_whole() {
    let (base, requests) = spawn(Arc::new(AtomicUsize::new(950)));
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let labels = labels(&github).await;

    assert_eq!(pages(&requests), (1..=10).collect::<Vec<_>>());
    assert_eq!(labels.len(), 950);
}

#[tokio::test]
async fn a_timeline_that_grew_behind_an_unchanged_first_page_is_still_read_to_its_end() {
    let total = Arc::new(AtomicUsize::new(1150));
    let (base, requests) = spawn(total.clone());
    let github = GitHub::with_base_uri("token", &base).unwrap();

    labels(&github).await;
    assert_eq!(pages(&requests), [1, 4, 5, 6, 7, 8, 9, 10, 11, 12]);

    // A hundred more events fill page 12 and start page 13. Page 1 holds
    // the same hundred, so it is a 304, and the `Link` kept with it still
    // says twelve.
    total.store(1250, Ordering::SeqCst);
    let labels = labels(&github).await;

    let tagged = requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.tagged)
        .collect::<Vec<_>>();
    assert_eq!(
        tagged,
        [[true; 10].as_slice(), &[false]].concat(),
        "only page 13 is new"
    );
    assert_eq!(pages(&requests), [1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
    assert_eq!(labels.len(), 100 + 800 + 50);
    assert_eq!(labels.last().map(String::as_str), Some("l1249"));
    assert!(
        !labels.contains(&"l399".to_string()),
        "page 4 fell out of the nine newest"
    );
    assert!(labels.contains(&"l400".to_string()));
}
