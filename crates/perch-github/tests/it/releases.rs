//! The latest release and its downloads against a local server, as the
//! updater asks for them.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use perch_github::{GitHub, ReleaseAsset};

/// Each request's path and `Accept` header, in the order they came.
type Requests = Arc<Mutex<Vec<(String, String)>>>;

/// A request's path and its `Accept` header.
fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let path = start.split_whitespace().nth(1)?.to_string();
    let mut accept = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim_end().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("accept") {
                accept = value.trim().to_string();
            }
        }
    }
    Some((path, accept))
}

fn respond(stream: &mut TcpStream, status: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// Answers `/releases/latest` with `latest` (a 404 when it is `None`) and
/// any other path with `file`, keeping each request's path and `Accept`.
fn spawn(latest: Option<String>, file: &'static [u8]) -> (String, Requests) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some((path, accept)) = read_request(&mut stream) else {
                continue;
            };
            let latest_asked = path.ends_with("/releases/latest");
            seen.lock().unwrap().push((path, accept));
            match (latest_asked, &latest) {
                (true, Some(body)) => respond(&mut stream, "200 OK", body.as_bytes()),
                (true, None) => {
                    respond(&mut stream, "404 Not Found", br#"{"message":"Not Found"}"#)
                }
                (false, _) => respond(&mut stream, "200 OK", file),
            }
        }
    });
    (format!("http://{addr}"), requests)
}

fn asset(base: &str, size: u64) -> ReleaseAsset {
    ReleaseAsset {
        id: 7,
        name: "Perch-macos-arm64.tar.gz".into(),
        size,
        url: format!("{base}/repos/perch-code/perch/releases/assets/7"),
    }
}

#[tokio::test]
async fn the_latest_release_lists_its_downloads() {
    let latest = r#"{"tag_name":"v0.2.0","html_url":"https://github.com/perch-code/perch/releases/tag/v0.2.0",
      "draft":false,"prerelease":false,
      "assets":[{"id":7,"name":"Perch-macos-arm64.tar.gz","size":5,
        "url":"https://api.github.com/repos/perch-code/perch/releases/assets/7",
        "browser_download_url":"https://github.com/perch-code/perch/releases/download/v0.2.0/Perch-macos-arm64.tar.gz"}]}"#;
    let (base, requests) = spawn(Some(latest.into()), b"");
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    let release = gh
        .latest_release("perch-code/perch")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(release.tag_name, "v0.2.0");
    let tarball = release.asset("Perch-macos-arm64.tar.gz").unwrap();
    assert_eq!((tarball.id, tarball.size), (7, 5));
    assert!(release.asset("Perch-macos-x86_64.tar.gz").is_none());

    let requests = requests.lock().unwrap();
    assert_eq!(requests[0].0, "/repos/perch-code/perch/releases/latest");
}

#[tokio::test]
async fn a_repository_with_no_release_has_no_latest() {
    let (base, _) = spawn(None, b"");
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    assert_eq!(gh.latest_release("perch-code/perch").await.unwrap(), None);
}

#[tokio::test]
async fn a_download_asks_for_the_file_itself() {
    let (base, requests) = spawn(None, b"perch");
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    let bytes = gh.download_release_asset(&asset(&base, 5)).await.unwrap();
    assert_eq!(bytes, b"perch");

    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.as_slice(),
        [(
            "/repos/perch-code/perch/releases/assets/7".to_string(),
            "application/octet-stream".to_string()
        )]
    );
}

#[tokio::test]
async fn a_download_short_of_the_listed_size_fails() {
    let (base, _) = spawn(None, b"per");
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    assert!(gh.download_release_asset(&asset(&base, 5)).await.is_err());
}
