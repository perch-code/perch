//! The discussion fetch against a local server, to pin pagination.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use perch_github::{GitHub, Label, PullEvent, PullEventKind, ReactionKind, Side};

/// The request's path, and its body: the reactions query names its ids
/// there.
fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let path = start.split_whitespace().nth(1)?.to_string();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim_end().is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    std::io::Read::read_exact(&mut reader, &mut body).ok()?;
    Some((path, String::from_utf8_lossy(&body).into_owned()))
}

/// One page of the timeline: a label, a commit, a review request, and
/// the merge that also closes the pull request, stamped a second after the
/// payload's `merged_at` as GitHub does. `mentioned` is sent to
/// everyone and shown to no one, and `reviewed` is already a card.
fn timeline() -> Vec<String> {
    vec![
        r#"{"event":"labeled","actor":{"login":"r"},"created_at":"2026-09-15T08:00:00Z","label":{"name":"bug","color":"d73a4a"}}"#.into(),
        r#"{"event":"committed","sha":"abc1234def","message":"Fence the manifest\n\nlonger","author":{"name":"Chris","date":"2026-09-15T09:00:00Z"}}"#.into(),
        r#"{"event":"review_requested","actor":{"login":"r"},"created_at":"2026-09-15T10:00:00Z","requested_reviewer":{"login":"a"}}"#.into(),
        r#"{"event":"mentioned","actor":{"login":"r"},"created_at":"2026-09-15T11:00:00Z"}"#.into(),
        r#"{"event":"closed","actor":{"login":"r"},"created_at":"2026-09-17T15:00:01Z"}"#.into(),
        r#"{"event":"merged","actor":{"login":"r"},"created_at":"2026-09-17T15:00:01Z","commit_id":"h"}"#.into(),
    ]
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
        r#"{{"id":{id},"node_id":"IC_{id}","user":{{"login":"u{id}"}},"body":"comment {id}","created_at":"2026-09-15T09:{:02}:00Z","html_url":"https://github.com/a/b/pull/1#issuecomment-{id}"}}"#,
        id % 60
    )
}

/// One reaction group as GraphQL writes it.
fn group(content: &str, count: u32, mine: bool) -> String {
    format!(r#"{{"content":"{content}","viewerHasReacted":{mine},"reactors":{{"totalCount":{count}}}}}"#)
}

/// The reactions answer for the ids a query named: the description has a
/// thumbs up of the reader's own, comment 1 has two hearts, and nothing
/// else has any.
fn reactions_for(body: &str) -> String {
    let ids: Vec<String> = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| serde_json::from_value(v["variables"]["ids"].clone()).ok())
        .unwrap_or_default();
    let nodes: Vec<String> = ids
        .iter()
        .map(|id| match id.as_str() {
            "PR_1" => format!(r#"{{"reactionGroups":[{}]}}"#, group("THUMBS_UP", 1, true)),
            "IC_1" => format!(
                r#"{{"reactionGroups":[{},{}]}}"#,
                group("THUMBS_UP", 0, false),
                group("HEART", 2, false)
            ),
            _ => r#"{"reactionGroups":[]}"#.into(),
        })
        .collect();
    format!(r#"{{"data":{{"nodes":[{}]}}}}"#, nodes.join(","))
}

/// The review threads answer: one thread, resolved, rooted at comment 3.
fn review_threads() -> String {
    r#"{"data":{"repository":{"pull_request":{"review_threads":{
        "page_info":{"has_next_page":false,"end_cursor":null},
        "nodes":[{"id":"PRRT_1","is_resolved":true,"is_outdated":false,
                  "comments":{"nodes":[{"database_id":3}]}}]}}}}}"#
        .into()
}

fn page(items: Vec<String>) -> String {
    format!("[{}]", items.join(","))
}

/// A pull request with 150 conversation comments, one review, and two line
/// comments, served a hundred a page, the state of its one review thread,
/// and the reactions on all of it. The ids each reactions query named are
/// kept, a query at a time.
#[allow(clippy::type_complexity)]
fn spawn(comments: usize) -> (String, Arc<AtomicUsize>, Arc<std::sync::Mutex<Vec<Vec<String>>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = asked.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some((path, request_body)) = read_request(&mut stream) else { continue };
            counter.fetch_add(1, Ordering::SeqCst);
            if path.starts_with("/graphql") && request_body.contains("reviewThreads") {
                respond(&mut stream, &review_threads());
                continue;
            }
            if path.starts_with("/graphql") {
                let ids: Vec<String> = serde_json::from_str::<serde_json::Value>(&request_body)
                    .ok()
                    .and_then(|v| serde_json::from_value(v["variables"]["ids"].clone()).ok())
                    .unwrap_or_default();
                seen.lock().unwrap().push(ids);
                respond(&mut stream, &reactions_for(&request_body));
                continue;
            }
            let page_no: usize = path
                .split("&page=")
                .nth(1)
                .and_then(|p| p.split('&').next())
                .and_then(|p| p.parse().ok())
                .unwrap_or(1);
            let body = if path.contains("/issues/1/timeline") {
                page(if page_no == 1 { timeline() } else { Vec::new() })
            } else if path.contains("/issues/1/comments") {
                let start = (page_no - 1) * 100;
                let end = (start + 100).min(comments);
                page((start..end).map(|i| comment(i + 1)).collect())
            } else if path.contains("/pulls/1/reviews") {
                page(vec![r#"{"id":9,"node_id":"PRR_9","user":{"login":"r"},"body":"lgtm","state":"APPROVED","submitted_at":"2026-09-16T11:40:00Z","html_url":"https://github.com/a/b/pull/1#pullrequestreview-9"}"#.into()])
            } else if path.contains("/pulls/1/commits") {
                page(vec![r#"{"sha":"abc1234def","author":{"login":"criccomini"}}"#.into()])
            } else if path.contains("/pulls/1/comments") {
                page(vec![
                    r#"{"id":3,"node_id":"PRRC_3","pull_request_review_id":9,"user":{"login":"r"},"body":"here","path":"src/x.rs","line":41,"side":"LEFT","diff_hunk":"@@ -40,3 +40,3 @@","created_at":"2026-09-16T11:38:00Z","html_url":"https://github.com/a/b/pull/1#discussion_r3"}"#.into(),
                    r#"{"id":4,"node_id":"PRRC_4","pull_request_review_id":9,"in_reply_to_id":3,"user":{"login":"a"},"body":"ok","path":"src/x.rs","line":41,"created_at":"2026-09-16T11:39:00Z","html_url":"https://github.com/a/b/pull/1#discussion_r4"}"#.into(),
                ])
            } else if path.contains("/pulls/1") {
                r#"{"number":1,"node_id":"PR_1","body":"The description.","created_at":"2026-09-14T14:02:00Z",
                    "merged":true,"merged_at":"2026-09-17T15:00:00Z","merged_by":{"login":"r"},
                    "closed_at":"2026-09-17T15:00:00Z","head":{"sha":"h"},"base":{"sha":"b"}}"#.into()
            } else {
                "[]".into()
            };
            respond(&mut stream, &body);
        }
    });
    (format!("http://{addr}"), requests, asked)
}

#[tokio::test]
async fn every_page_of_the_conversation_is_read() {
    let (base, requests, asked) = spawn(150);
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
    // The opening out of the pull request's own payload, since the
    // timeline has no line for it, and the rest out of the timeline. The
    // merge is said once: the payload's and the close beside it are both
    // the same moment, though GitHub stamps them a second apart.
    let at = |s: &str| s.parse::<jiff::Timestamp>().unwrap();
    assert_eq!(
        d.events,
        vec![
            PullEvent::new(PullEventKind::Opened, at("2026-09-14T14:02:00Z"), ""),
            PullEvent::new(
                PullEventKind::Labeled {
                    label: Label { name: "bug".into(), color: "d73a4a".into() }
                },
                at("2026-09-15T08:00:00Z"),
                "r",
            ),
            PullEvent::new(
                PullEventKind::Committed {
                    sha: "abc1234def".into(),
                    author: "Chris".into(),
                    // The timeline sent no email that says whose it is,
                    // and the commit list does.
                    login: "criccomini".into(),
                    subject: "Fence the manifest".into(),
                },
                at("2026-09-15T09:00:00Z"),
                "",
            ),
            PullEvent::new(
                PullEventKind::ReviewRequested { who: "a".into() },
                at("2026-09-15T10:00:00Z"),
                "r",
            ),
            PullEvent::new(PullEventKind::Merged, at("2026-09-17T15:00:01Z"), "r"),
        ]
    );
    // The reactions, on everything at once: the description, 150 comments,
    // the review and its two line comments are 154 ids, asked for a
    // hundred at a time.
    assert_eq!(d.node_id, "PR_1");
    assert!(d.reactions.viewer_reacted(ReactionKind::ThumbsUp));
    assert_eq!(d.comments[0].reactions.count(ReactionKind::Heart), 2);
    assert!(!d.comments[0].reactions.viewer_reacted(ReactionKind::Heart));
    assert_eq!(d.comments[0].reactions.count(ReactionKind::ThumbsUp), 0, "nobody's is not shown");
    assert!(d.comments[1].reactions.is_empty());
    assert!(d.reviews[0].reactions.is_empty());
    assert_eq!(d.review_comments[0].node_id, "PRRC_3");
    let asked = asked.lock().unwrap();
    assert_eq!(asked.iter().map(Vec::len).collect::<Vec<_>>(), vec![100, 54]);
    assert_eq!(asked[0][0], "PR_1");
    assert_eq!(asked[1][53], "PRRC_4");
    // The state of the one review thread, by its first comment.
    assert_eq!(d.review_threads.len(), 1);
    assert_eq!(d.review_threads[0].node_id, "PRRT_1");
    assert_eq!(d.review_threads[0].root_comment_id, 3);
    assert!(d.review_threads[0].resolved);
    assert!(!d.review_threads[0].outdated);
    // The pull request, two comment pages, one reviews page, one line page,
    // one timeline page, one commits page, the review threads query, and
    // two reactions queries.
    assert_eq!(requests.load(Ordering::SeqCst), 10);
}

#[tokio::test]
async fn an_exact_page_costs_one_more_empty_request_and_nothing_else() {
    let (base, _, _) = spawn(100);
    let gh = GitHub::with_base_uri("token", &base).unwrap();
    let d = gh.fetch_discussion("a/b", 1).await.unwrap();
    assert_eq!(d.comments.len(), 100);
}
