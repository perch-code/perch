//! API requests do not wait on each other one at a time: an open asking
//! for an issue is not held behind whatever the pollers asked first.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use perch_github::GitHub;

fn read_path(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut start = String::new();
    reader.read_line(&mut start).unwrap();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 || line.trim_end().is_empty() {
            break;
        }
    }
    start.split_whitespace().nth(1).unwrap().to_string()
}

/// Take two requests before answering either. A client that sends one at
/// a time never sends the second.
fn spawn_holding_two() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let path = read_path(&mut stream);
            held.push((stream, path));
        }
        for (mut stream, path) in held {
            let number = path.rsplit('/').next().unwrap();
            let body = format!(r#"{{"number":{number},"updated_at":"2026-09-16T11:40:00Z"}}"#);
            let wire = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(wire.as_bytes()).unwrap();
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn two_requests_are_in_flight_at_once() {
    let base = spawn_holding_two();
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let both = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(github.fetch_issue("a/b", 1), github.fetch_issue("a/b", 2))
    })
    .await
    .expect("the second request went out before the first was answered");

    assert_eq!(both.0.unwrap().number, 1);
    assert_eq!(both.1.unwrap().number, 2);
}
