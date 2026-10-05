//! A description edited from Perch, or a pull request closed or merged
//! from it, is read again from GitHub, not from a read of the pull
//! request kept from moments before.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use perch_github::{CommentPlace, GitHub, MergeMethod};

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    if_none_match: Option<String>,
    body: String,
}

/// A 200 with its body and the tag to send with it, or none.
struct Reply {
    etag: Option<&'static str>,
    body: String,
}

impl Reply {
    fn ok(body: impl Into<String>) -> Self {
        Self {
            etag: None,
            body: body.into(),
        }
    }

    fn tagged(etag: &'static str, body: impl Into<String>) -> Self {
        Self {
            etag: Some(etag),
            body: body.into(),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let mut words = start.split_whitespace();
    let method = words.next()?.to_string();
    let path = words.next()?.to_string();
    let mut content_length = 0usize;
    let mut if_none_match = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim_end().is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("if-none-match") {
                if_none_match = Some(value.trim().to_string());
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        if_none_match,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// Answer each request as `route` says, each on its own thread, so one
/// `route` holds back does not hold back the rest. A request asked with
/// the tag its answer would carry gets a 304, as GitHub's would.
fn spawn(
    route: impl Fn(&Request) -> Reply + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let route = Arc::new(route);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (route, seen) = (route.clone(), seen.clone());
            std::thread::spawn(move || {
                let Some(request) = read_request(&mut stream) else {
                    return;
                };
                let reply = route(&request);
                let unchanged =
                    reply.etag.is_some() && reply.etag == request.if_none_match.as_deref();
                let etag = reply
                    .etag
                    .map(|etag| format!("ETag: {etag}\r\n"))
                    .unwrap_or_default();
                let wire = if unchanged {
                    format!("HTTP/1.1 304 Not Modified\r\n{etag}Connection: close\r\n\r\n")
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{etag}Connection: close\r\n\r\n{}",
                        reply.body.len(),
                        reply.body,
                    )
                };
                seen.lock().unwrap().push(request);
                let _ = stream.write_all(wire.as_bytes());
                let _ = stream.flush();
            });
        }
    });
    (format!("http://{address}"), requests)
}

/// The pull request as GitHub sends it, with `body` for its description.
fn pull(body: &str) -> String {
    format!(
        r#"{{"number":7,"node_id":"PR_7","body":"{body}","created_at":"2026-10-01T10:00:00Z","head":{{"sha":"h"}},"base":{{"sha":"b"}}}}"#
    )
}

/// A pull request whose description becomes "After." once its edit is
/// taken. GitHub names its repository `Octo/Perch` in the edit's answer,
/// cased unlike the `octo/perch` the reader asked for it under.
fn serve_an_edit() -> (String, Arc<Mutex<Vec<Request>>>) {
    serve_an_edit_holding(None)
}

/// `serve_an_edit`, with the first read of the pull request held, when
/// `hold` is given, until the test lets it go: `hold` says when it is
/// asked, and is told when to answer. It answers with the description
/// as it was before the edit, as GitHub would have when it was asked.
fn serve_an_edit_holding(
    hold: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
) -> (String, Arc<Mutex<Vec<Request>>>) {
    let edited = AtomicBool::new(false);
    let hold = Mutex::new(hold);
    spawn(move |request| {
        if request.path == "/graphql" && request.body.contains("updatePullRequest") {
            edited.store(true, Ordering::SeqCst);
            return Reply::ok(
                r#"{"data":{"updatePullRequest":{"pullRequest":{"body":"After.","number":7,"repository":{"nameWithOwner":"Octo/Perch"},"updatedAt":"2026-10-04T12:00:00Z"}}}}"#,
            );
        }
        if request.path == "/graphql" {
            return Reply::ok(r#"{"data":{"nodes":[]}}"#);
        }
        if request.path == "/repos/octo/perch/pulls/7" {
            let held = hold.lock().unwrap().take();
            if let Some((asked, answer)) = held {
                let _ = asked.send(());
                let _ = answer.recv();
                return Reply::tagged(r#""before""#, pull("Before."));
            }
            return if edited.load(Ordering::SeqCst) {
                Reply::tagged(r#""after""#, pull("After."))
            } else {
                Reply::tagged(r#""before""#, pull("Before."))
            };
        }
        Reply::ok("[]")
    })
}

fn pull_reads(requests: &Mutex<Vec<Request>>) -> usize {
    requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.method == "GET" && r.path == "/repos/octo/perch/pulls/7")
        .count()
}

#[tokio::test]
async fn a_discussion_read_right_after_the_pull_request_takes_that_read_as_current() {
    let (base, requests) = serve_an_edit();
    let github = GitHub::with_base_uri("token", &base).unwrap();

    github.fetch_pull_request("octo/perch", 7).await.unwrap();
    let discussion = github.fetch_discussion("octo/perch", 7).await.unwrap();

    assert_eq!(discussion.body, "Before.");
    assert_eq!(pull_reads(&requests), 1, "the read of a moment ago stood");
}

#[tokio::test]
async fn a_discussion_read_right_after_a_description_edit_asks_github_for_the_new_one() {
    let (base, requests) = serve_an_edit();
    let github = GitHub::with_base_uri("token", &base).unwrap();

    github.fetch_pull_request("octo/perch", 7).await.unwrap();
    let kept = github
        .update_comment("PR_7", CommentPlace::PullRequestBody, "After.")
        .await
        .unwrap();
    assert_eq!(kept.body, "After.");
    assert_eq!(
        kept.updated_at,
        Some("2026-10-04T12:00:00Z".parse().unwrap())
    );
    let discussion = github.fetch_discussion("octo/perch", 7).await.unwrap();

    assert_eq!(discussion.body, "After.");
    assert_eq!(
        pull_reads(&requests),
        2,
        "the discussion asked GitHub again"
    );
    let requests = requests.lock().unwrap();
    let edit = requests.iter().find(|r| r.path == "/graphql").unwrap();
    assert!(edit
        .body
        .contains("pullRequest { body number repository { nameWithOwner } updatedAt }"));
}

/// A read of the pull request asked before its description was edited,
/// and answered after, says what it said before. Its answer is not kept,
/// so the discussion read after the edit asks GitHub rather than taking
/// it as current.
#[tokio::test]
async fn a_pull_request_read_asked_before_a_description_edit_is_not_kept_as_current() {
    let (asked, was_asked) = std::sync::mpsc::channel();
    let (answer, to_answer) = std::sync::mpsc::channel();
    let (base, requests) = serve_an_edit_holding(Some((asked, to_answer)));
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let early = tokio::spawn({
        let github = github.clone();
        async move { github.fetch_pull_request("octo/perch", 7).await }
    });
    tokio::task::spawn_blocking(move || was_asked.recv().unwrap())
        .await
        .unwrap();
    github
        .update_comment("PR_7", CommentPlace::PullRequestBody, "After.")
        .await
        .unwrap();
    answer.send(()).unwrap();
    early.await.unwrap().unwrap();
    let discussion = github.fetch_discussion("octo/perch", 7).await.unwrap();

    assert_eq!(discussion.body, "After.");
    assert_eq!(
        pull_reads(&requests),
        2,
        "the discussion asked GitHub again"
    );
}

/// A pull request whose description becomes "Done." once it is closed
/// or merged, under any casing of its repository's name.
fn serve_a_close() -> (String, Arc<Mutex<Vec<Request>>>) {
    let done = Arc::new(AtomicBool::new(false));
    spawn(move |request| {
        let path = request.path.to_ascii_lowercase();
        if request.method == "PATCH" && path == "/repos/octo/perch/pulls/7" {
            done.store(true, Ordering::SeqCst);
            return Reply::ok(pull("Done."));
        }
        if request.method == "PUT" && path == "/repos/octo/perch/pulls/7/merge" {
            done.store(true, Ordering::SeqCst);
            return Reply::ok(r#"{"sha":"m","merged":true,"message":"Merged"}"#);
        }
        if request.path == "/graphql" {
            return Reply::ok(r#"{"data":{"nodes":[]}}"#);
        }
        if request.path == "/repos/octo/perch/pulls/7" {
            return if done.load(Ordering::SeqCst) {
                Reply::tagged(r#""done""#, pull("Done."))
            } else {
                Reply::tagged(r#""before""#, pull("Before."))
            };
        }
        Reply::ok("[]")
    })
}

#[tokio::test]
async fn a_discussion_read_right_after_a_close_or_a_merge_asks_github_again() {
    for merge in [false, true] {
        let (base, requests) = serve_a_close();
        let github = GitHub::with_base_uri("token", &base).unwrap();

        github.fetch_pull_request("octo/perch", 7).await.unwrap();
        if merge {
            github
                .merge_pull_request("Octo/Perch", 7, "h", MergeMethod::Squash, None)
                .await
                .unwrap();
        } else {
            github.close_pull_request("Octo/Perch", 7).await.unwrap();
        }
        let discussion = github.fetch_discussion("octo/perch", 7).await.unwrap();

        assert_eq!(discussion.body, "Done.", "merge: {merge}");
        assert_eq!(pull_reads(&requests), 2, "merge: {merge}");
    }
}
