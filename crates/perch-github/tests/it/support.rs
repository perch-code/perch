//! Helpers that more than one test module uses: scripted local servers
//! that stand in for GitHub, and pieces of the JSON it answers.

use std::io::{BufRead, BufReader};
use std::net::TcpStream;

/// A server for GraphQL: each request's path and its body as JSON.
pub mod graphql {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    pub struct Request {
        pub path: String,
        pub body: serde_json::Value,
    }

    pub struct Response {
        status: &'static str,
        body: String,
    }

    impl Response {
        pub fn ok(body: impl Into<String>) -> Self {
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

    pub fn spawn(responses: Vec<Response>) -> (String, Arc<Mutex<Vec<Request>>>) {
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
}

/// A server for REST: each request's method, path and body as text.
pub mod rest {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    pub struct Request {
        pub method: String,
        pub path: String,
        pub body: String,
    }

    pub struct Response {
        status: &'static str,
        location: Option<&'static str>,
        body: String,
    }

    impl Response {
        pub fn new(status: &'static str, body: impl Into<String>) -> Self {
            Self {
                status,
                location: None,
                body: body.into(),
            }
        }

        /// GitHub's answer to a write on a repository's old name.
        pub fn moved(location: &'static str) -> Self {
            Self {
                location: Some(location),
                ..Self::new(
                    "307 Temporary Redirect",
                    r#"{"message":"Moved Permanently"}"#,
                )
            }
        }
    }

    fn read_request(stream: &mut TcpStream) -> Option<Request> {
        let mut reader = BufReader::new(stream.try_clone().ok()?);
        let mut start = String::new();
        reader.read_line(&mut start).ok()?;
        let mut start = start.split_whitespace();
        let method = start.next()?.to_string();
        let path = start.next()?.to_string();
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
            method,
            path,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }

    fn respond(stream: &mut TcpStream, response: Response) {
        let location = response
            .location
            .map(|location| format!("Location: {location}\r\n"))
            .unwrap_or_default();
        let wire = format!(
            "HTTP/1.1 {status}\r\n{location}Content-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}",
            status = response.status,
            length = response.body.len(),
            body = response.body,
        );
        stream.write_all(wire.as_bytes()).unwrap();
        stream.flush().unwrap();
    }

    pub fn spawn(responses: Vec<Response>) -> (String, Arc<Mutex<Vec<Request>>>) {
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
}

/// Reads a request's start line and headers, and answers its path.
pub fn read_path(stream: &mut TcpStream) -> Option<String> {
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

/// A JSON array of items that are JSON already.
pub fn page(items: Vec<String>) -> String {
    format!("[{}]", items.join(","))
}

/// One reaction group as GraphQL answers it.
pub fn group(content: &str, count: u32, mine: bool) -> String {
    format!(
        r#"{{"content":"{content}","viewerHasReacted":{mine},"reactors":{{"totalCount":{count}}}}}"#
    )
}
