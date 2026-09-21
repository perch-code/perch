//! Reactions against a scripted local server: what the client asks
//! GraphQL, and what it makes of the answer.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::{Error, GitHub, ReactionKind};

#[derive(Debug)]
struct Request {
    path: String,
    body: serde_json::Value,
}

struct Response {
    status: &'static str,
    body: String,
}

impl Response {
    fn ok(body: impl Into<String>) -> Self {
        Self {
            status: "200 OK",
            body: body.into(),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let path = start.split_whitespace().nth(1)?.to_string();
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
        path,
        body: serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
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

fn group(content: &str, count: u32, mine: bool) -> String {
    format!(
        r#"{{"content":"{content}","viewerHasReacted":{mine},"reactors":{{"totalCount":{count}}}}}"#
    )
}

#[tokio::test]
async fn adding_a_reaction_names_the_subject_and_reads_it_back() {
    let (base, requests) = spawn(vec![Response::ok(format!(
        r#"{{"data":{{"addReaction":{{"subject":{{"reactionGroups":[{},{}]}}}}}}}}"#,
        group("THUMBS_UP", 3, true),
        group("EYES", 0, false),
    ))]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let reactions = github
        .set_reaction("IC_1", ReactionKind::ThumbsUp, true)
        .await
        .unwrap();
    assert_eq!(reactions.count(ReactionKind::ThumbsUp), 3);
    assert!(reactions.viewer_reacted(ReactionKind::ThumbsUp));
    assert_eq!(reactions.groups.len(), 1, "a kind nobody left is not shown");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/graphql");
    let query = requests[0].body["query"].as_str().unwrap();
    assert!(query.starts_with("mutation("), "{query}");
    assert!(
        query.contains("addReaction(input: {subjectId: $subject, content: $content})"),
        "{query}"
    );
    assert!(query.contains("subject { reactionGroups {"), "{query}");
    assert_eq!(requests[0].body["variables"]["subject"], "IC_1");
    assert_eq!(requests[0].body["variables"]["content"], "THUMBS_UP");
}

#[tokio::test]
async fn taking_a_reaction_back_is_the_other_mutation() {
    let (base, requests) = spawn(vec![Response::ok(
        r#"{"data":{"removeReaction":{"subject":{"reactionGroups":[]}}}}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let reactions = github
        .set_reaction("PRRC_3", ReactionKind::Heart, false)
        .await
        .unwrap();
    assert!(reactions.is_empty());

    let requests = requests.lock().unwrap();
    let query = requests[0].body["query"].as_str().unwrap();
    assert!(query.contains("removeReaction(input:"), "{query}");
    assert_eq!(requests[0].body["variables"]["content"], "HEART");
}

#[tokio::test]
async fn graphql_errors_and_a_missing_subject_are_errors() {
    let (base, _) = spawn(vec![
        Response::ok(
            r#"{"data":null,"errors":[{"message":"Resource not accessible by integration"}]}"#,
        ),
        Response::ok(r#"{"data":{"addReaction":null}}"#),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let refused = github
        .set_reaction("IC_1", ReactionKind::Rocket, true)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, Error::Graphql(message) if message.contains("not accessible")),
        "{refused:?}"
    );
    let empty = github
        .set_reaction("IC_1", ReactionKind::Rocket, true)
        .await
        .unwrap_err();
    assert!(matches!(empty, Error::Decode(_)), "{empty:?}");

    // No id, no request.
    let none = github
        .set_reaction("", ReactionKind::Rocket, true)
        .await
        .unwrap_err();
    assert!(matches!(none, Error::Decode(_)), "{none:?}");
}

#[tokio::test]
async fn reactions_are_read_a_hundred_nodes_at_a_time_and_a_missing_node_is_left_out() {
    let first: Vec<String> = (0..100)
        .map(|i| {
            if i == 7 {
                format!(r#"{{"reactionGroups":[{}]}}"#, group("ROCKET", 1, true))
            } else {
                r#"{"reactionGroups":[]}"#.to_string()
            }
        })
        .collect();
    let (base, requests) = spawn(vec![
        Response::ok(format!(r#"{{"data":{{"nodes":[{}]}}}}"#, first.join(","))),
        Response::ok(r#"{"data":{"nodes":[null,{"reactionGroups":[]}]}}"#),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let ids: Vec<String> = (0..102).map(|i| format!("N_{i}")).collect();
    let reactions = github.fetch_reactions(&ids).await.unwrap();
    assert_eq!(
        reactions.len(),
        101,
        "one of the hundred and two was not resolved"
    );
    assert!(reactions["N_7"].viewer_reacted(ReactionKind::Rocket));
    assert!(reactions["N_0"].is_empty());
    assert!(!reactions.contains_key("N_100"));
    assert!(reactions["N_101"].is_empty());

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let asked = |i: usize| {
        requests[i].body["variables"]["ids"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!((asked(0), asked(1)), (100, 2));
    assert!(requests[0].body["query"]
        .as_str()
        .unwrap()
        .contains("nodes(ids: $ids)"));
}

#[tokio::test]
async fn no_ids_costs_no_request() {
    let (base, requests) = spawn(vec![]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    let reactions = github.fetch_reactions(&[]).await.unwrap();
    assert!(reactions.is_empty());
    assert!(requests.lock().unwrap().is_empty());
}
