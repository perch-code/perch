//! The repository lists against a local server: the conditional first
//! page, the pull requests an issues page carries, the `Link` header, the
//! exact closed queries, and the allowance every answer leaves behind.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use jiff::Timestamp;
use perch_github::{Allowance, Error, GitHub, Listed};

const ETAG: &str = "W/\"6c2b0e\"";
const RATE: &str = "X-RateLimit-Remaining: 4321\r\nX-RateLimit-Reset: 1790000000\r\n";

/// A list entry as `GET /repos/{r}/pulls` sends it: no additions,
/// deletions or changed files, which the list omits.
fn pull_json(n: u64, state: &str) -> String {
    format!(
        r#"{{"number":{n},"title":"pull {n}","state":"{state}","draft":false,
            "user":{{"login":"u{n}","avatar_url":"https://avatars/{n}"}},
            "head":{{"sha":"h{n}","ref":"branch-{n}","repo":{{"full_name":"a/b"}}}},
            "base":{{"sha":"m","ref":"main","repo":{{"full_name":"a/b"}}}},
            "updated_at":"2026-09-16T11:40:00Z"}}"#
    )
}

fn issue_json(n: u64) -> String {
    format!(
        r#"{{"number":{n},"title":"issue {n}","body":"","state":"open",
            "user":{{"login":"u{n}"}},"comments":0,
            "created_at":"2026-09-14T14:02:00Z","updated_at":"2026-09-16T11:40:00Z"}}"#
    )
}

/// The issues endpoint's shape for a pull request: an issue plus a
/// `pull_request` key.
fn pull_as_issue_json(n: u64) -> String {
    format!(
        r#"{{"number":{n},"title":"pull {n}","body":"","state":"open",
            "user":{{"login":"u{n}"}},"comments":0,"updated_at":"2026-09-16T11:40:00Z",
            "pull_request":{{"url":"https://api.github.com/repos/a/b/pulls/{n}"}}}}"#
    )
}

fn page(items: Vec<String>) -> String {
    format!("[{}]", items.join(","))
}

/// The `page` parameter of a request path, 1 when absent.
fn page_of(path: &str) -> u64 {
    path.split("&page=")
        .nth(1)
        .and_then(|p| p.split('&').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or(1)
}

#[derive(Clone, Debug)]
struct Request {
    path: String,
    if_none_match: Option<String>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    if start.trim().is_empty() {
        return None;
    }
    let path = start.split_whitespace().nth(1)?.to_string();

    let mut if_none_match = None;
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
            if name == "if-none-match" {
                if_none_match = Some(value);
            } else if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
        }
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).ok()?;
    }

    Some(Request { path, if_none_match })
}

fn write_response(stream: &mut TcpStream, status: &str, extra: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         {extra}\
         Connection: close\r\n\
         \r\n{body}",
        len = body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// What the server says back: the status line, extra header lines each
/// ending in `\r\n`, and the body.
struct Answer {
    status: &'static str,
    extra: String,
    body: String,
}

fn ok(extra: impl Into<String>, body: String) -> Answer {
    Answer { status: "200 OK", extra: extra.into(), body }
}

/// A server that answers every request through `answer` and remembers
/// what it was asked, in order.
fn spawn<F>(answer: F) -> (String, Arc<Mutex<Vec<Request>>>)
where
    F: Fn(&Request) -> Answer + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests = Arc::new(Mutex::new(Vec::new()));

    let seen = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(req) = read_request(&mut stream) else { continue };
            seen.lock().unwrap().push(req.clone());
            let Answer { status, extra, body } = answer(&req);
            write_response(&mut stream, status, &extra, &body);
        }
    });

    (format!("http://{addr}"), requests)
}

fn paths(requests: &Arc<Mutex<Vec<Request>>>) -> Vec<String> {
    requests.lock().unwrap().iter().map(|r| r.path.clone()).collect()
}

#[tokio::test]
async fn a_304_is_unchanged_and_still_updates_the_allowance() {
    let (base, requests) = spawn(|req| {
        if req.if_none_match.as_deref() == Some(ETAG) {
            Answer { status: "304 Not Modified", extra: RATE.into(), body: String::new() }
        } else {
            ok(format!("ETag: {ETAG}\r\n{RATE}"), page(vec![pull_json(1, "open")]))
        }
    });
    let gh = GitHub::with_base_uri("token", &base).expect("client");
    assert_eq!(gh.allowance(), None, "nothing has been seen before the first request");

    let pulls = gh.list_open_pulls("a/b", Some(ETAG)).await.expect("list pulls");
    assert_eq!(pulls, Listed::Unchanged, "a 304 means the rows from last time still stand");
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "a 304 on the first page must not be followed by another"
    );
    assert_eq!(
        gh.allowance(),
        Some(Allowance { remaining: 4321, reset_epoch: 1790000000 }),
        "a 304 carries the rate limit headers and they must be kept"
    );

    let issues = gh.list_open_issues("a/b", Some(ETAG)).await.expect("list issues");
    assert_eq!(issues, Listed::Unchanged, "the issues list answers a 304 the same way");
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn only_the_first_page_is_conditional_and_a_list_past_one_page_leaves_no_tag() {
    // 250 open pull requests: two full pages and a short third, each with
    // a tag of its own, as GitHub sends them.
    let (base, requests) = spawn(|req| {
        let n = page_of(&req.path);
        let start = (n - 1) * 100;
        let end = (start + 100).min(250);
        let extra = if n == 1 { format!("ETag: {ETAG}\r\n{RATE}") } else { format!("ETag: W/\"page-{n}\"\r\n{RATE}") };
        ok(extra, page((start..end).map(|i| pull_json(i + 1, "open")).collect()))
    });
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let listed = gh
        .list_open_pulls("a/b", Some("W/\"stale\""))
        .await
        .expect("list pulls");

    let Listed::Changed { etag, items } = listed else {
        panic!("a 200 is a change");
    };
    assert_eq!(items.len(), 250, "every page is read to the first short one");
    assert_eq!(items[0].repo_full_name, "a/b");
    // A tag vouches for one page's body. A closure on page 2 leaves page
    // 1 as it was, so page 1's tag would answer 304 over it; a list that
    // ran past one page hands back no tag, and the next pass asks whole.
    assert_eq!(etag, None, "no page's tag vouches for a list longer than that page");

    assert_eq!(
        paths(&requests),
        [
            "/repos/a/b/pulls?state=open&per_page=100&page=1",
            "/repos/a/b/pulls?state=open&per_page=100&page=2",
            "/repos/a/b/pulls?state=open&per_page=100&page=3",
        ]
    );
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests[0].if_none_match.as_deref(),
        Some("W/\"stale\""),
        "the first page asks with the stored tag"
    );
    assert!(
        requests[1..].iter().all(|r| r.if_none_match.is_none()),
        "a later page is never conditional: {requests:?}"
    );
}

#[tokio::test]
async fn a_list_that_fits_on_one_page_hands_back_that_page_s_tag() {
    let (base, requests) = spawn(|_| ok(format!("ETag: {ETAG}\r\n{RATE}"), page(vec![pull_json(1, "open")])));
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let listed = gh.list_open_pulls("a/b", None).await.expect("list pulls");

    let Listed::Changed { etag, items } = listed else {
        panic!("a 200 is a change");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(etag.as_deref(), Some(ETAG), "one page is the whole list, and its tag vouches for all of it");
    assert_eq!(requests.lock().unwrap().len(), 1, "a short first page is the last");
}

#[tokio::test]
async fn a_first_page_without_a_tag_leaves_none_to_store() {
    let (base, requests) = spawn(|_| ok(RATE, page(vec![pull_json(1, "open")])));
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let listed = gh.list_open_pulls("a/b", None).await.expect("list pulls");

    let Listed::Changed { etag, items } = listed else {
        panic!("a 200 is a change");
    };
    assert_eq!(etag, None, "no ETag means the next pass asks unconditionally");
    assert_eq!(items.len(), 1);
    assert!(
        requests.lock().unwrap()[0].if_none_match.is_none(),
        "no stored tag, no conditional header"
    );
}

#[tokio::test]
async fn an_issues_page_drops_pull_requests_without_shortening_the_page() {
    // Page 1 is a hundred entries, twenty of them pull requests; page 2 is short.
    let (base, requests) = spawn(|req| match page_of(&req.path) {
        1 => ok(
            format!("ETag: {ETAG}\r\n{RATE}"),
            page(
                (1..=100)
                    .map(|n| if n % 5 == 0 { pull_as_issue_json(n) } else { issue_json(n) })
                    .collect(),
            ),
        ),
        n => ok(format!("ETag: W/\"page-{n}\"\r\n{RATE}"), page(vec![issue_json(101)])),
    });
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let listed = gh.list_open_issues("a/b", None).await.expect("list issues");

    let Listed::Changed { etag, items } = listed else {
        panic!("a 200 is a change");
    };
    assert_eq!(etag, None, "two pages were read, so no one tag vouches for the list");
    assert_eq!(items.len(), 81, "eighty issues from page 1 and one from page 2");
    assert!(
        items.iter().all(|i| i.number % 5 != 0),
        "an entry with a pull_request key is not an issue"
    );
    assert!(items.iter().all(|i| i.repo_full_name == "a/b"));
    assert_eq!(
        paths(&requests),
        [
            "/repos/a/b/issues?state=open&per_page=100&page=1",
            "/repos/a/b/issues?state=open&per_page=100&page=2",
        ],
        "a full page of entries is a full page whatever it held"
    );
}

#[tokio::test]
async fn the_link_header_yields_the_last_page_and_its_absence_yields_none() {
    let (base, _) = spawn(|req| {
        let closed: Vec<String> = (1..=100).map(|n| pull_json(n, "closed")).collect();
        if req.path.contains("/issues?") {
            // A repository whose closed issues fit on one page: no Link at all.
            return ok(RATE, page(vec![issue_json(9)]));
        }
        match page_of(&req.path) {
            7 => ok(
                format!(
                    "Link: <http://x/repos/a/b/pulls?state=closed&sort=updated&direction=asc&per_page=100&page=6>; rel=\"prev\", \
                     <http://x/repos/a/b/pulls?state=closed&sort=updated&direction=asc&per_page=100&page=1>; rel=\"first\"\r\n{RATE}"
                ),
                page(vec![pull_json(700, "closed")]),
            ),
            _ => ok(
                format!(
                    "Link: <http://x/repos/a/b/pulls?state=closed&sort=updated&direction=asc&per_page=100&page=2>; rel=\"next\", \
                     <http://x/repos/a/b/pulls?state=closed&sort=updated&direction=asc&per_page=100&page=7>; rel=\"last\"\r\n{RATE}"
                ),
                page(closed),
            ),
        }
    });
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let first = gh.list_closed_pulls("a/b", 1).await.expect("first page");
    assert_eq!(first.last_page, Some(7), "rel=last names the page the history ends on");
    assert_eq!(first.items.len(), 100);
    assert!(!first.items[0].is_open());

    let last = gh.list_closed_pulls("a/b", 7).await.expect("last page");
    assert_eq!(
        last.last_page, None,
        "the last page has no rel=last, and that is how the backfill knows it arrived"
    );
    assert_eq!(last.items[0].number, 700);

    let issues = gh.list_closed_issues("a/b", 1, None).await.expect("issues");
    assert_eq!(issues.last_page, None, "no Link header at all means one page");
    assert_eq!(issues.items[0].number, 9);
}

#[tokio::test]
async fn the_closed_queries_carry_sort_direction_and_since() {
    let (base, requests) = spawn(|_| ok(RATE, "[]".to_string()));
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    gh.list_closed_pulls("a/b", 3).await.expect("pulls");
    gh.list_closed_issues("a/b", 2, None).await.expect("issues");
    let since: Timestamp = "2026-09-14T14:02:00Z".parse().unwrap();
    gh.list_closed_issues("a/b", 1, Some(since)).await.expect("issues since");

    assert_eq!(
        paths(&requests),
        [
            "/repos/a/b/pulls?state=closed&sort=updated&direction=asc&per_page=100&page=3",
            "/repos/a/b/issues?state=closed&sort=updated&direction=asc&per_page=100&page=2",
            "/repos/a/b/issues?state=closed&sort=updated&direction=asc&per_page=100&page=1&since=2026-09-14T14:02:00Z",
        ],
        "oldest updated first is what makes an interrupted run resumable"
    );
}

#[tokio::test]
async fn a_closed_issues_page_drops_pull_requests_too_but_its_newest_counts_them() {
    // The pull request is the page's last entry, updated after both issues.
    let later = pull_as_issue_json(2).replace("2026-09-16T11:40:00Z", "2026-09-17T08:00:00Z");
    let (base, _) = spawn(move |_| ok(RATE, page(vec![issue_json(1), later.clone(), issue_json(3)])));
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let listed = gh.list_closed_issues("a/b", 1, None).await.expect("issues");

    let numbers: Vec<u64> = listed.items.iter().map(|i| i.number).collect();
    assert_eq!(numbers, [1, 3], "history has no place for a pull request filed as an issue");
    assert_eq!(
        listed.newest,
        Some("2026-09-17T08:00:00Z".parse().unwrap()),
        "but the page was read past it, and a since cursor short of it would list it every time"
    );

    let (base, _) = spawn(|_| ok(RATE, page(vec![pull_json(4, "closed"), pull_json(5, "closed")])));
    let gh = GitHub::with_base_uri("token", &base).expect("client");
    let pulls = gh.list_closed_pulls("a/b", 1).await.expect("pulls");
    assert_eq!(pulls.newest, pulls.items[1].updated_at, "a pull request page's newest is its entries'");
}

#[tokio::test]
async fn a_403_whose_body_names_the_rate_limit_is_rate_limited() {
    let (base, _) = spawn(|_| Answer {
        status: "403 Forbidden",
        extra: "X-RateLimit-Remaining: 12\r\nX-RateLimit-Reset: 1790000000\r\n".into(),
        body: r#"{"message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again."}"#.into(),
    });
    let gh = GitHub::with_base_uri("token", &base).expect("client");

    let err = gh.list_open_pulls("a/b", None).await.expect_err("a 403 is an error");

    assert!(
        matches!(err, Error::RateLimited { reset_epoch: Some(1790000000) }),
        "the body names the limit even though remaining is not zero: {err:?}"
    );
    assert_eq!(
        gh.allowance(),
        Some(Allowance { remaining: 12, reset_epoch: 1790000000 }),
        "a refusal carries the headers and they must be kept too"
    );
}
