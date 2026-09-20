//! GitHub REST client: the notifications inbox and targeted item lookups.
//!
//! Uses octocrab's raw `_get_with_headers` rather than its typed builders,
//! because polling needs `X-Poll-Interval` and `Last-Modified`, targeted
//! branch lookup uses `ETag`, and rate-limit headers classify failures.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{
    HeaderMap, HeaderName, HeaderValue, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use http::StatusCode;
use http_body_util::BodyExt;
use jiff::Timestamp;
use octocrab::Octocrab;

use crate::discussion::{
    Discussion, IssueComment, IssueCommentPayload, PullSummary, Review, ReviewComment,
    ReviewCommentPayload, ReviewPayload,
};
use crate::items::{Issue, IssuePayload};
use crate::model::Thread;
use crate::pulls::{PullPayload, PullRequest};
use crate::workflow_runs::{WorkflowRunPayload, WorkflowRunsPayload};

/// Page size for the notifications list. GitHub caps `per_page` at 50 here.
const PER_PAGE: usize = 50;
/// Fallback when the server sends no `X-Poll-Interval`.
const DEFAULT_POLL_SECS: u64 = 60;
/// GitHub's dated REST contract. Pinning this keeps a future default change
/// from silently changing response shapes underneath the client.
const API_VERSION: &str = "2022-11-28";
/// Page size for discussion timelines. GitHub allows a hundred there.
const LIST_PER_PAGE: usize = 100;
/// Hard cap on pages for discussion lists.
const LIST_MAX_PAGES: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GitHub rejected the token")]
    Unauthorized,
    #[error("rate limited")]
    RateLimited {
        reset_epoch: Option<i64>,
        retry_after_secs: Option<u64>,
    },
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
    Answer { pull: Option<Box<PullRequest>>, etag: Option<String> },
}

/// Outcome of a notifications poll.
#[derive(Debug, Clone, PartialEq)]
pub struct ListResult {
    /// The complete inbox when `unchanged` is false. Empty when the
    /// conditional probe returned 304 and the cached inbox still stands.
    pub threads: Vec<Thread>,
    pub unchanged: bool,
    /// The validator to use for the next poll.
    pub last_modified: Option<String>,
    pub poll_interval: Duration,
}

/// One raw `/notifications` page response, before its body is decoded.
struct NotificationPage {
    status: StatusCode,
    last_modified: Option<String>,
    poll_interval: Duration,
    body: String,
}

/// One response with its headers read before the body is consumed.
struct Fetched {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

#[derive(Clone)]
pub struct GitHub {
    octo: Octocrab,
    http: reqwest::Client,
    /// Serializes API requests across clones so a response that closes the
    /// rate-limit window gates the next request before it is sent.
    api_request_lock: Arc<tokio::sync::Mutex<()>>,
    /// Shared by every clone so one rate-limit response gates all later API
    /// calls, including item fetches spawned outside the poll loop.
    rate_limit_until: Arc<AtomicI64>,
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
        let mut builder = Octocrab::builder()
            .personal_token(token.to_string())
            .add_header(
                HeaderName::from_static("x-github-api-version"),
                API_VERSION.to_string(),
            );
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

        Ok(Self {
            octo,
            http,
            api_request_lock: Arc::new(tokio::sync::Mutex::new(())),
            rate_limit_until: Arc::new(AtomicI64::new(0)),
        })
    }

    /// Fetch the complete inbox unconditionally.
    ///
    /// This wrapper is useful for callers without a cached validator. Polling
    /// callers should use [`Self::list_threads_since`].
    pub async fn list_threads(&self) -> Result<ListResult, Error> {
        self.list_threads_since(None).await
    }

    /// Fetch the authenticated user's GitHub notifications inbox.
    ///
    /// Read and unread threads are both requested because github.com's Inbox
    /// is one list with an Unread filter. `last_modified` is used only for a
    /// conditional request. A 304 keeps the cached inbox; a 200 is the new
    /// complete first page and is followed by any remaining pages.
    pub async fn list_threads_since(
        &self,
        last_modified: Option<&str>,
    ) -> Result<ListResult, Error> {
        let mut first_page = None;
        if let Some(last_modified) = last_modified {
            let probe = self.fetch_notification_page(1, Some(last_modified)).await?;
            if probe.status == StatusCode::NOT_MODIFIED {
                return Ok(ListResult {
                    threads: Vec::new(),
                    unchanged: true,
                    last_modified: probe
                        .last_modified
                        .or_else(|| Some(last_modified.to_string())),
                    poll_interval: probe.poll_interval,
                });
            }
            first_page = Some(probe);
        }

        let mut threads = Vec::new();
        let mut page = 1usize;
        let mut response_last_modified = None;
        let mut poll_interval = Duration::from_secs(DEFAULT_POLL_SECS);

        loop {
            let response = match first_page.take() {
                Some(response) if page == 1 => response,
                _ => self.fetch_notification_page(page, None).await?,
            };
            if page == 1 {
                response_last_modified = response.last_modified.clone();
                poll_interval = response.poll_interval;
            } else {
                poll_interval = poll_interval.max(response.poll_interval);
            }

            let page_threads: Vec<Thread> = serde_json::from_str(&response.body)
                .map_err(|e| Error::Decode(e.to_string()))?;
            let is_last_page = page_threads.len() < PER_PAGE;
            threads.extend(page_threads);
            if is_last_page {
                break;
            }
            page = page
                .checked_add(1)
                .ok_or_else(|| Error::Decode("notification page number overflowed".into()))?;
        }

        Ok(ListResult {
            threads,
            unchanged: false,
            last_modified: response_last_modified,
            poll_interval,
        })
    }

    /// Mark one notification read, the same operation as opening it in
    /// github.com's notifications inbox.
    pub async fn mark_thread_read(&self, thread_id: &str) -> Result<(), Error> {
        let path = notification_thread_path(thread_id, "")?;
        let _request_guard = self.api_request_lock.lock().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._patch(&path, None::<&()>)
            .await
            .map_err(map_octocrab_error)?;
        self.finish_mutation(response, &[StatusCode::RESET_CONTENT, StatusCode::NOT_MODIFIED])
            .await
    }

    /// Remove one notification from github.com's inbox as Done.
    pub async fn mark_thread_done(&self, thread_id: &str) -> Result<(), Error> {
        let path = notification_thread_path(thread_id, "")?;
        let _request_guard = self.api_request_lock.lock().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._delete(&path, None::<&()>)
            .await
            .map_err(map_octocrab_error)?;
        self.finish_mutation(response, &[StatusCode::NO_CONTENT]).await
    }

    /// Ignore future activity on a notification thread, including when the
    /// user watches its repository.
    pub async fn ignore_thread(&self, thread_id: &str) -> Result<(), Error> {
        let path = notification_thread_path(thread_id, "/subscription")?;
        let body = serde_json::json!({ "ignored": true });
        let _request_guard = self.api_request_lock.lock().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._put(&path, Some(&body))
            .await
            .map_err(map_octocrab_error)?;
        self.finish_mutation(response, &[StatusCode::OK, StatusCode::NOT_MODIFIED])
            .await
    }

    /// One page of `/notifications`, including the headers that pace and
    /// validate polling.
    async fn fetch_notification_page(
        &self,
        page: usize,
        last_modified: Option<&str>,
    ) -> Result<NotificationPage, Error> {
        let uri = format!(
            "/notifications?all=true&participating=false&per_page={PER_PAGE}&page={page}"
        );
        let fetched = self.get(&uri, modified_since(last_modified)).await?;
        let last_modified = header_str(&fetched.headers, LAST_MODIFIED.as_str());
        let poll_interval = header_str(&fetched.headers, "x-poll-interval")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|seconds| Duration::from_secs(seconds.max(1)))
            .unwrap_or_else(|| Duration::from_secs(DEFAULT_POLL_SECS));

        Ok(NotificationPage {
            status: fetched.status,
            last_modified,
            poll_interval,
            body: fetched.body,
        })
    }

    /// Fetch a pull request by number.
    ///
    /// `repo_full_name` is the repository it was asked for under, used only
    /// when the payload omits its own base repository.
    pub async fn fetch_pull_request(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<PullRequest, Error> {
        let path = format!("/repos/{repo_full_name}/pulls/{number}");
        let fetched = self.get(&path, None).await?;
        let payload: PullPayload =
            serde_json::from_str(&fetched.body).map_err(|e| Error::Decode(e.to_string()))?;

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
    pub async fn fetch_issue(&self, repo_full_name: &str, number: u64) -> Result<Issue, Error> {
        let body = self
            .get_text(&format!("/repos/{repo_full_name}/issues/{number}"))
            .await?;
        let payload: IssuePayload =
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

        let fetched = self.get(&path, conditional(etag)).await?;
        if fetched.status == StatusCode::NOT_MODIFIED {
            return Ok(PullLookup::Unchanged);
        }
        let new_etag = header_str(&fetched.headers, ETAG.as_str());
        let payloads: Vec<PullPayload> =
            serde_json::from_str(&fetched.body).map_err(|e| Error::Decode(e.to_string()))?;
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
        let mut all = Vec::new();
        for page in 1..=LIST_MAX_PAGES {
            let joiner = if path.contains('?') { '&' } else { '?' };
            let body = self
                .get_text(&format!("{path}{joiner}per_page={LIST_PER_PAGE}&page={page}"))
                .await?;
            let items: Vec<T> =
                serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
            let short = items.len() < LIST_PER_PAGE;
            all.extend(items);
            if short {
                break;
            }
        }
        Ok(all)
    }

    /// One GET whose body is all the caller wants.
    async fn get_text(&self, path: &str) -> Result<String, Error> {
        Ok(self.get(path, None).await?.body)
    }

    /// One GET, classified the way every request is.
    ///
    /// A 2xx comes back with its body; a 304 comes back with an empty one,
    /// for the callers that asked conditionally; anything else is an error.
    /// Rate-limit response headers are still used to classify an error.
    async fn get(&self, path: &str, headers: Option<HeaderMap>) -> Result<Fetched, Error> {
        let _request_guard = self.api_request_lock.lock().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._get_with_headers(path, headers)
            .await
            .map_err(map_octocrab_error)?;

        let status = response.status();
        let headers = response.headers().clone();
        let (remaining, reset, retry_after) = rate_headers(&headers);
        self.remember_rate_headers(remaining, reset, retry_after);

        if status == StatusCode::NOT_MODIFIED {
            return Ok(Fetched { status, headers, body: String::new() });
        }

        let body = body_text(response).await;
        if !status.is_success() {
            let error = classify(status, remaining, reset, retry_after, body);
            self.remember_rate_limit(&error);
            return Err(error);
        }
        Ok(Fetched { status, headers, body })
    }

    /// Classify a mutation response with the same error behavior as GETs.
    /// Some notification endpoints use 304 as a
    /// successful idempotent result, so accepted statuses are explicit.
    async fn finish_mutation<B>(
        &self,
        response: http::Response<B>,
        accepted: &[StatusCode],
    ) -> Result<(), Error>
    where
        B: http_body::Body<Data = Bytes>,
    {
        let status = response.status();
        let headers = response.headers().clone();
        let (remaining, reset, retry_after) = rate_headers(&headers);
        self.remember_rate_headers(remaining, reset, retry_after);
        let body = body_text(response).await;

        if accepted.contains(&status) {
            return Ok(());
        }
        let error = classify(status, remaining, reset, retry_after, body);
        self.remember_rate_limit(&error);
        Err(error)
    }

    fn check_rate_limit(&self) -> Result<(), Error> {
        let until = self.rate_limit_until.load(Ordering::SeqCst);
        if until > Timestamp::now().as_second() {
            return Err(Error::RateLimited {
                reset_epoch: Some(until),
                retry_after_secs: None,
            });
        }
        Ok(())
    }

    fn remember_rate_headers(
        &self,
        remaining: Option<i64>,
        reset_epoch: Option<i64>,
        retry_after_secs: Option<u64>,
    ) {
        if remaining == Some(0) || retry_after_secs.is_some() {
            self.remember_rate_limit(&Error::RateLimited {
                reset_epoch,
                retry_after_secs,
            });
        }
    }

    fn remember_rate_limit(&self, error: &Error) {
        let Error::RateLimited {
            reset_epoch,
            retry_after_secs,
        } = error
        else {
            return;
        };
        let until = rate_limit_deadline(*reset_epoch, *retry_after_secs);
        self.rate_limit_until.fetch_max(until, Ordering::SeqCst);
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

/// The request headers for a conditional GET: `If-None-Match` when there
/// is a tag to ask with, nothing otherwise.
fn conditional(etag: Option<&str>) -> Option<HeaderMap> {
    let value = etag.and_then(|e| HeaderValue::from_str(e).ok())?;
    let mut headers = HeaderMap::new();
    headers.insert(IF_NONE_MATCH, value);
    Some(headers)
}

/// The conditional header for a notifications probe.
fn modified_since(last_modified: Option<&str>) -> Option<HeaderMap> {
    let value = last_modified.and_then(|value| HeaderValue::from_str(value).ok())?;
    let mut headers = HeaderMap::new();
    headers.insert(IF_MODIFIED_SINCE, value);
    Some(headers)
}

/// Build a notification endpoint from the numeric ID GitHub supplied.
fn notification_thread_path(thread_id: &str, suffix: &str) -> Result<String, Error> {
    if thread_id.is_empty() || !thread_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::Decode(format!(
            "notification thread id is not numeric: {thread_id}"
        )));
    }
    Ok(format!("/notifications/threads/{thread_id}{suffix}"))
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

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

/// Requests remaining, the primary-window reset epoch, and a secondary-limit
/// delay. Any value is `None` when its header is absent or unreadable.
fn rate_headers(headers: &HeaderMap) -> (Option<i64>, Option<i64>, Option<u64>) {
    let remaining =
        header_str(headers, "x-ratelimit-remaining").and_then(|v| v.parse::<i64>().ok());
    let reset = header_str(headers, "x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
    let retry_after = header_str(headers, "retry-after").and_then(|v| v.parse::<u64>().ok());
    (remaining, reset, retry_after)
}

fn rate_limit_deadline(reset_epoch: Option<i64>, retry_after_secs: Option<u64>) -> i64 {
    let now = Timestamp::now().as_second();
    if let Some(seconds) = retry_after_secs {
        let seconds = i64::try_from(seconds.max(1)).unwrap_or(i64::MAX);
        return now.saturating_add(seconds);
    }
    reset_epoch.filter(|reset| *reset > now).unwrap_or_else(|| now.saturating_add(60))
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
    retry_after_secs: Option<u64>,
    body: String,
) -> Error {
    if status == StatusCode::UNAUTHORIZED {
        return Error::Unauthorized;
    }
    // A spent window leaves `remaining` at zero. A secondary limit does not,
    // and says so only in the body, so the body is read for it too.
    let exhausted = rate_remaining == Some(0) || body.to_ascii_lowercase().contains("rate limit");
    if (status == StatusCode::FORBIDDEN && exhausted) || status == StatusCode::TOO_MANY_REQUESTS {
        return Error::RateLimited {
            reset_epoch: rate_reset,
            retry_after_secs,
        };
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
    fn unauthorized_is_distinct_from_rate_limiting() {
        assert!(matches!(
            classify(StatusCode::UNAUTHORIZED, None, None, None, String::new()),
            Error::Unauthorized
        ));
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, Some(0), Some(123), None, String::new()),
            Error::RateLimited { reset_epoch: Some(123), retry_after_secs: None }
        ));
        // A 403 that is not a rate limit stays a plain status error.
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, Some(42), None, None, "no".into()),
            Error::Status { status: 403, .. }
        ));
    }

    #[test]
    fn a_403_that_names_the_rate_limit_is_rate_limited_whatever_remains() {
        let body = r#"{"message":"You have exceeded a secondary rate limit."}"#.to_string();
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, Some(42), Some(9), Some(120), body),
            Error::RateLimited {
                reset_epoch: Some(9),
                retry_after_secs: Some(120)
            }
        ));
    }

    #[tokio::test]
    async fn retry_after_sets_a_fail_fast_shared_gate_and_wins_over_reset() {
        let gh = GitHub::new("token").unwrap();
        let before = Timestamp::now().as_second();
        gh.remember_rate_limit(&Error::RateLimited {
            reset_epoch: Some(before + 60),
            retry_after_secs: Some(3_600),
        });
        assert!(gh.rate_limit_until.load(Ordering::SeqCst) >= before + 3_600);
        assert!(matches!(
            gh.check_rate_limit(),
            Err(Error::RateLimited {
                reset_epoch: Some(reset),
                retry_after_secs: None
            }) if reset >= before + 3_600
        ));
    }

}
