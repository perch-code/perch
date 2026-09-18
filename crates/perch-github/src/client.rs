//! GitHub REST client for the notifications inbox.
//!
//! Uses octocrab's raw `_get_with_headers` rather than its typed notification
//! builder, because the polling rules need response headers: `X-Poll-Interval`
//! sets the floor on how often we may ask again, `Last-Modified` drives
//! conditional requests, and the rate limit headers drive backoff.

use std::time::Duration;

use bytes::Bytes;
use http::header::{HeaderMap, HeaderValue, IF_MODIFIED_SINCE, LAST_MODIFIED};
use http::StatusCode;
use http_body_util::BodyExt;
use octocrab::Octocrab;

use crate::discussion::{
    Discussion, IssueComment, IssueCommentPayload, PullSummary, Review, ReviewComment,
    ReviewCommentPayload, ReviewPayload,
};
use crate::model::{SubjectDetail, SubjectKind, SubjectPayload, Thread};
use crate::workflow_runs::{WorkflowRunPayload, WorkflowRunQuery, WorkflowRunsPayload};

/// Page size for the notifications list. GitHub caps `per_page` at 50 here.
const PER_PAGE: usize = 50;
/// Hard cap on pages, so a huge inbox can't spin forever. 10 x 50 = 500.
const MAX_PAGES: usize = 10;
/// Fallback when the server sends no `X-Poll-Interval`.
const DEFAULT_POLL_SECS: u64 = 60;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GitHub rejected the token")]
    Unauthorized,
    #[error("rate limited")]
    RateLimited { reset_epoch: Option<i64> },
    #[error("GitHub returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("could not parse GitHub's response: {0}")]
    Decode(String),
}

/// What asking after a branch's pull request gave back.
#[derive(Debug, Clone, PartialEq)]
pub enum PullLookup {
    /// A 304: whatever was known still holds.
    Unchanged,
    /// A fresh answer, and the tag to ask with next time.
    Answer { pull: Option<Box<crate::pulls::PullRequest>>, etag: Option<String> },
}

/// Outcome of a notifications poll.
#[derive(Debug)]
pub struct ListResult {
    /// `None` means the server answered 304 and our cached list still stands.
    pub threads: Option<Vec<Thread>>,
    pub last_modified: Option<String>,
    pub poll_interval: Duration,
}

/// One raw page response, before it is parsed.
struct Page {
    status: StatusCode,
    last_modified: Option<String>,
    poll_interval: Duration,
    body: String,
}

#[derive(Clone)]
pub struct GitHub {
    octo: Octocrab,
    http: reqwest::Client,
}

impl GitHub {
    pub fn new(token: &str) -> Result<Self, Error> {
        Self::build(token, None)
    }

    /// Point the client at a different API root. Used by tests to stand up a
    /// local server in place of api.github.com.
    pub fn with_base_uri(token: &str, base_uri: &str) -> Result<Self, Error> {
        Self::build(token, Some(base_uri))
    }

    fn build(token: &str, base_uri: Option<&str>) -> Result<Self, Error> {
        let mut builder = Octocrab::builder().personal_token(token.to_string());
        if let Some(base) = base_uri {
            builder = builder
                .base_uri(base)
                .map_err(|e| Error::Network(e.to_string()))?;
        }
        let octo = builder
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;

        let http = reqwest::Client::builder()
            .user_agent(concat!("perch/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;

        Ok(Self { octo, http })
    }

    /// Fetch the inbox.
    ///
    /// Always asks for read and unread threads: the web UI's Unread tab is a
    /// filter over one list, not a separate query.
    ///
    /// `since` is the previous response's `Last-Modified`, used only as a
    /// cheap "has anything changed?" probe. It is deliberately *not* used to
    /// build the returned list, because GitHub treats `If-Modified-Since` on
    /// this endpoint as a filter rather than a cache validator: a 200 carries
    /// only the threads updated since that instant. A partial list cannot
    /// show which threads were dismissed on github.com or unsubscribed from,
    /// so acting on one would silently drop the rest of the inbox. When the
    /// probe says something moved, we pay one extra request for the whole
    /// list.
    ///
    /// Perch's own Done is a separate thing entirely, and local: see
    /// `cache::Mark`.
    pub async fn list_threads(&self, since: Option<&str>) -> Result<ListResult, Error> {
        if let Some(since) = since {
            let probe = self.fetch_page(1, Some(since)).await?;
            if probe.status == StatusCode::NOT_MODIFIED {
                return Ok(ListResult {
                    threads: None,
                    last_modified: probe.last_modified,
                    poll_interval: probe.poll_interval,
                });
            }
            // Something changed. Fall through and re-read the full inbox.
        }

        let mut all = Vec::new();
        let mut last_modified = None;
        let mut poll_interval = Duration::from_secs(DEFAULT_POLL_SECS);

        for page in 1..=MAX_PAGES {
            let response = self.fetch_page(page, None).await?;

            if page == 1 {
                last_modified = response.last_modified.clone();
                poll_interval = response.poll_interval;
            }

            let page_threads: Vec<Thread> = serde_json::from_str(&response.body)
                .map_err(|e| Error::Decode(e.to_string()))?;

            let short_page = page_threads.len() < PER_PAGE;
            all.extend(page_threads);
            if short_page {
                break;
            }
        }

        Ok(ListResult {
            threads: Some(all),
            last_modified,
            poll_interval,
        })
    }

    /// One page of `/notifications`, with the headers the poll loop needs.
    async fn fetch_page(&self, page: usize, since: Option<&str>) -> Result<Page, Error> {
        let uri =
            format!("/notifications?all=true&participating=false&per_page={PER_PAGE}&page={page}");

        let mut headers = HeaderMap::new();
        if let Some(since) = since.and_then(|s| HeaderValue::from_str(s).ok()) {
            headers.insert(IF_MODIFIED_SINCE, since);
        }

        let response = self
            .octo
            ._get_with_headers(&uri, Some(headers))
            .await
            .map_err(map_octocrab_error)?;

        let status = response.status();
        let last_modified = header_str(response.headers(), LAST_MODIFIED.as_str());
        let poll_interval = header_str(response.headers(), "x-poll-interval")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|s| Duration::from_secs(s.max(1)))
            .unwrap_or_else(|| Duration::from_secs(DEFAULT_POLL_SECS));
        let rate_reset =
            header_str(response.headers(), "x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
        let rate_remaining = header_str(response.headers(), "x-ratelimit-remaining")
            .and_then(|v| v.parse::<i64>().ok());

        if status == StatusCode::NOT_MODIFIED {
            return Ok(Page {
                status,
                last_modified,
                poll_interval,
                body: String::new(),
            });
        }

        let body = body_text(response).await;
        if !status.is_success() {
            return Err(classify(status, rate_remaining, rate_reset, body));
        }

        Ok(Page {
            status,
            last_modified,
            poll_interval,
            body,
        })
    }

    /// Fetch the issue or PR behind a thread: number, state, author.
    pub async fn fetch_detail(
        &self,
        subject_url: &str,
        kind: &SubjectKind,
    ) -> Result<SubjectDetail, Error> {
        let path = api_path(subject_url).ok_or_else(|| {
            Error::Decode(format!("subject url is not a GitHub API url: {subject_url}"))
        })?;

        let response = self
            .octo
            ._get_with_headers(&path, None)
            .await
            .map_err(map_octocrab_error)?;

        let status = response.status();
        let rate_reset =
            header_str(response.headers(), "x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
        let rate_remaining = header_str(response.headers(), "x-ratelimit-remaining")
            .and_then(|v| v.parse::<i64>().ok());

        if !status.is_success() {
            let body = body_text(response).await;
            return Err(classify(status, rate_remaining, rate_reset, body));
        }

        let body = body_text(response).await;
        let payload: SubjectPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;

        Ok(payload.into_detail(kind))
    }

    /// Find the workflow run behind a `CheckSuite` notification.
    ///
    /// `Ok(None)` when no run on the branch fits: it has aged out of GitHub's
    /// retention, or the title named something the branch's runs do not.
    pub async fn fetch_workflow_run_detail(
        &self,
        query: &WorkflowRunQuery,
    ) -> Result<Option<SubjectDetail>, Error> {
        let body = self.get_text(&query.path()).await?;
        let payload: WorkflowRunsPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(query.pick(payload.workflow_runs))
    }

    /// Fetch the pull request behind a notification.
    ///
    /// `repo_full_name` is the repository the notification came from, used
    /// only when the payload omits its own base repository.
    pub async fn fetch_pull_request(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<crate::pulls::PullRequest, Error> {
        let path = format!("/repos/{repo_full_name}/pulls/{number}");
        let response = self
            .octo
            ._get_with_headers(&path, None)
            .await
            .map_err(map_octocrab_error)?;

        let status = response.status();
        let rate_reset =
            header_str(response.headers(), "x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
        let rate_remaining = header_str(response.headers(), "x-ratelimit-remaining")
            .and_then(|v| v.parse::<i64>().ok());

        if !status.is_success() {
            let body = body_text(response).await;
            return Err(classify(status, rate_remaining, rate_reset, body));
        }

        let body = body_text(response).await;
        let payload: crate::pulls::PullPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;

        Ok(payload.into_pull_request(repo_full_name))
    }

    /// The workflow runs for one commit, newest first. What the checks
    /// pill is set from. One page: a head with more than a hundred runs is
    /// not a head anyone is reviewing.
    pub async fn list_workflow_runs_for_sha(
        &self,
        repo_full_name: &str,
        head_sha: &str,
    ) -> Result<Vec<WorkflowRunPayload>, Error> {
        let body = self
            .get_text(&format!("/repos/{repo_full_name}/actions/runs?head_sha={head_sha}&per_page=100"))
            .await?;
        let payload: WorkflowRunsPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(payload.workflow_runs)
    }

    /// Everything said on a pull request: description, conversation,
    /// reviews, and line comments. Three lists, each read to the end, and
    /// the events the pull request's own payload carries.
    pub async fn fetch_discussion(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<Discussion, Error> {
        let base = format!("/repos/{repo_full_name}");
        let body = self.get_text(&format!("{base}/pulls/{number}")).await?;
        let pull: PullSummary =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;

        let comments: Vec<IssueCommentPayload> =
            self.list_all(&format!("{base}/issues/{number}/comments")).await?;
        let reviews: Vec<ReviewPayload> =
            self.list_all(&format!("{base}/pulls/{number}/reviews")).await?;
        let review_comments: Vec<ReviewCommentPayload> =
            self.list_all(&format!("{base}/pulls/{number}/comments")).await?;

        Ok(Discussion {
            opened_at: pull.created_at,
            events: pull.events(),
            body: pull.body.unwrap_or_default(),
            comments: comments.into_iter().map(IssueComment::from).collect(),
            reviews: reviews.into_iter().map(Review::from).collect(),
            review_comments: review_comments.into_iter().map(ReviewComment::from).collect(),
        })
    }

    /// Fetch an issue by number.
    ///
    /// The issues endpoint answers for pull requests too, because GitHub
    /// numbers the two together. One is refused here, so nothing that calls
    /// this can store a pull request as an issue.
    pub async fn fetch_issue(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<crate::items::Issue, Error> {
        let body = self
            .get_text(&format!("/repos/{repo_full_name}/issues/{number}"))
            .await?;
        let payload: crate::items::IssuePayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        if payload.is_pull_request() {
            return Err(Error::Decode(format!("{repo_full_name}#{number} is a pull request")));
        }
        Ok(payload.into_issue(repo_full_name))
    }

    /// Who the token belongs to: the login and the avatar, for the
    /// reader's own byline.
    pub async fn viewer(&self) -> Result<crate::model::UserPayload, Error> {
        let body = self.get_text("/user").await?;
        serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))
    }

    /// The conversation on an issue, read to the end.
    pub async fn fetch_issue_comments(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<Vec<IssueComment>, Error> {
        let comments: Vec<IssueCommentPayload> = self
            .list_all(&format!("/repos/{repo_full_name}/issues/{number}/comments"))
            .await?;
        Ok(comments.into_iter().map(IssueComment::from).collect())
    }

    /// The pull request whose head is `owner:branch` on `repo_full_name`,
    /// open or closed, with a conditional request: an unchanged answer
    /// costs a 304 and returns `Unchanged`.
    pub async fn find_pull_for_branch(
        &self,
        repo_full_name: &str,
        head_owner: &str,
        branch: &str,
        etag: Option<&str>,
    ) -> Result<PullLookup, Error> {
        let head_raw = format!("{head_owner}:{branch}");
        let head = percent_encoding::utf8_percent_encode(&head_raw, percent_encoding::NON_ALPHANUMERIC);
        let path = format!("/repos/{repo_full_name}/pulls?head={head}&state=all&per_page=5");

        let mut headers = HeaderMap::new();
        if let Some(etag) = etag.and_then(|e| HeaderValue::from_str(e).ok()) {
            headers.insert(http::header::IF_NONE_MATCH, etag);
        }
        let response = self
            .octo
            ._get_with_headers(&path, Some(headers))
            .await
            .map_err(map_octocrab_error)?;
        let status = response.status();
        if status == StatusCode::NOT_MODIFIED {
            return Ok(PullLookup::Unchanged);
        }
        let new_etag = header_str(response.headers(), "etag");
        let rate_reset =
            header_str(response.headers(), "x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
        let rate_remaining = header_str(response.headers(), "x-ratelimit-remaining")
            .and_then(|v| v.parse::<i64>().ok());
        let body = body_text(response).await;
        if !status.is_success() {
            return Err(classify(status, rate_remaining, rate_reset, body));
        }
        let payloads: Vec<crate::pulls::PullPayload> =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        // Open first, else the newest: a branch that had a pull request
        // closed and reopened as another should show the live one.
        let pick = payloads
            .iter()
            .position(|p| p.state.eq_ignore_ascii_case("open"))
            .or(if payloads.is_empty() { None } else { Some(0) });
        let pull = pick.map(|i| {
            let mut all = payloads;
            Box::new(all.swap_remove(i).into_pull_request(repo_full_name))
        });
        Ok(PullLookup::Answer { pull, etag: new_etag })
    }

    /// Every page of a list endpoint, concatenated.
    ///
    /// Stops at the first short page rather than following `Link` headers,
    /// which is what `list_threads` does too. A hundred a page and a
    /// hundred pages is more discussion than any pull request has.
    async fn list_all<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, Error> {
        const PER_PAGE: usize = 100;
        const MAX_PAGES: usize = 100;
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            let joiner = if path.contains('?') { '&' } else { '?' };
            let body = self
                .get_text(&format!("{path}{joiner}per_page={PER_PAGE}&page={page}"))
                .await?;
            let items: Vec<T> =
                serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
            let short = items.len() < PER_PAGE;
            all.extend(items);
            if short {
                break;
            }
        }
        Ok(all)
    }

    /// One GET, classified the way every other request is.
    async fn get_text(&self, path: &str) -> Result<String, Error> {
        let response = self
            .octo
            ._get_with_headers(path, None)
            .await
            .map_err(map_octocrab_error)?;

        let status = response.status();
        let rate_reset =
            header_str(response.headers(), "x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
        let rate_remaining = header_str(response.headers(), "x-ratelimit-remaining")
            .and_then(|v| v.parse::<i64>().ok());

        let body = body_text(response).await;
        if !status.is_success() {
            return Err(classify(status, rate_remaining, rate_reset, body));
        }
        Ok(body)
    }

    /// Download an avatar at display size. Returns raw encoded bytes.
    pub async fn fetch_avatar(&self, url: &str) -> Result<Vec<u8>, Error> {
        let sized = with_size_param(url, 64);
        let response = self
            .http
            .get(&sized)
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(Error::Status {
                status: response.status().as_u16(),
                body: "avatar fetch failed".into(),
            });
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| Error::Network(e.to_string()))
    }
}

/// GitHub serves avatars at whatever size `s` asks for. Asking for 64 keeps a
/// 20pt circle crisp on a retina display without pulling the full image.
fn with_size_param(url: &str, size: u32) -> String {
    if url.contains('?') {
        format!("{url}&s={size}")
    } else {
        format!("{url}?s={size}")
    }
}

/// Reduce an absolute `api.github.com` URL to the path octocrab expects.
fn api_path(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://api.github.com")
        .or_else(|| url.strip_prefix("http://api.github.com"))?;
    Some(rest.to_string())
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

async fn body_text(response: http::Response<impl http_body::Body<Data = Bytes>>) -> String {
    match response.into_body().collect().await {
        Ok(collected) => String::from_utf8_lossy(&collected.to_bytes()).into_owned(),
        Err(_) => String::new(),
    }
}

fn classify(
    status: StatusCode,
    rate_remaining: Option<i64>,
    rate_reset: Option<i64>,
    body: String,
) -> Error {
    if status == StatusCode::UNAUTHORIZED {
        return Error::Unauthorized;
    }
    let exhausted = rate_remaining == Some(0);
    if (status == StatusCode::FORBIDDEN && exhausted) || status == StatusCode::TOO_MANY_REQUESTS {
        return Error::RateLimited { reset_epoch: rate_reset };
    }
    Error::Status {
        status: status.as_u16(),
        body: body.chars().take(300).collect(),
    }
}

fn map_octocrab_error(e: octocrab::Error) -> Error {
    match &e {
        octocrab::Error::GitHub { source, .. } if source.status_code == StatusCode::UNAUTHORIZED => {
            Error::Unauthorized
        }
        _ => Error::Network(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_urls_gain_a_size_parameter() {
        assert_eq!(with_size_param("https://x/a.png", 64), "https://x/a.png?s=64");
        assert_eq!(with_size_param("https://x/a?v=4", 64), "https://x/a?v=4&s=64");
    }

    #[test]
    fn api_urls_reduce_to_paths() {
        assert_eq!(
            api_path("https://api.github.com/repos/slatedb/slatedb/pulls/2091").as_deref(),
            Some("/repos/slatedb/slatedb/pulls/2091")
        );
        assert_eq!(api_path("https://example.com/nope"), None);
    }

    #[test]
    fn unauthorized_is_distinct_from_rate_limiting() {
        assert!(matches!(
            classify(StatusCode::UNAUTHORIZED, None, None, String::new()),
            Error::Unauthorized
        ));
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, Some(0), Some(123), String::new()),
            Error::RateLimited { reset_epoch: Some(123) }
        ));
        // A 403 that is not a rate limit stays a plain status error.
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, Some(42), None, "no".into()),
            Error::Status { status: 403, .. }
        ));
    }
}
