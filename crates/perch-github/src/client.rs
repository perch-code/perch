//! GitHub REST client: the notifications inbox and the repository lists.
//!
//! Uses octocrab's raw `_get_with_headers` rather than its typed builders,
//! because the rules need response headers: `X-Poll-Interval` sets the
//! floor on how often the inbox may be asked again, `Last-Modified` and
//! `ETag` drive conditional requests, `Link` says how long a repository's
//! closed history is, and the rate limit headers are kept as the allowance
//! the backfill paces itself by.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{
    HeaderMap, HeaderValue, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, LINK,
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
use crate::model::{SubjectDetail, SubjectKind, SubjectPayload, Thread};
use crate::pulls::{PullPayload, PullRequest};
use crate::workflow_runs::{WorkflowRunPayload, WorkflowRunQuery, WorkflowRunsPayload};

/// Page size for the notifications list. GitHub caps `per_page` at 50 here.
const PER_PAGE: usize = 50;
/// Hard cap on pages, so a huge inbox can't spin forever. 10 x 50 = 500.
const MAX_PAGES: usize = 10;
/// Fallback when the server sends no `X-Poll-Interval`.
const DEFAULT_POLL_SECS: u64 = 60;
/// Page size for every other list. GitHub allows a hundred there.
const LIST_PER_PAGE: usize = 100;
/// Hard cap on pages for those lists. A hundred pages of a hundred is more
/// discussion than any pull request has, and more open pull requests than
/// any repository keeps.
const LIST_MAX_PAGES: usize = 100;
/// What an allowance cell holds before any response has been seen.
const UNSEEN: i64 = -1;

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
    Answer { pull: Option<Box<PullRequest>>, etag: Option<String> },
}

/// What listing a repository's open pull requests or issues gave back.
#[derive(Debug, Clone, PartialEq)]
pub enum Listed<T> {
    /// A 304 on the first page: nothing opened, closed, or moved since the
    /// tag was taken, so the rows made from the last answer still stand.
    Unchanged,
    /// The whole list, and the tag to ask with next time: the first
    /// page's when the list fit on one page, because only the first page
    /// is asked conditionally and its tag vouches for that page alone;
    /// `None` when the list ran on, so the next ask is unconditional.
    Changed { etag: Option<String>, items: Vec<T> },
}

/// One page of a repository's closed history, read a page at a time so the
/// backfill can stop and resume by page number.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// The page the `Link` header names as `rel="last"`. Absent on the last
    /// page itself, and when the whole list fits in one.
    pub last_page: Option<u32>,
    /// The newest `updated_at` among everything the page listed, the pull
    /// requests an issues page carried and dropped included. The list is
    /// oldest updated first, so this is where the page was consumed to,
    /// and where a later `since` must start from, whether or not the last
    /// entry survived into `items`.
    pub newest: Option<Timestamp>,
}

/// The rate limit as the last response reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allowance {
    /// Requests left in the current window.
    pub remaining: i64,
    /// When the window resets, in seconds since the Unix epoch.
    pub reset_epoch: i64,
}

/// Outcome of a notifications poll.
#[derive(Debug)]
pub struct ListResult {
    /// `None` means the server answered 304 and our cached list still stands.
    pub threads: Option<Vec<Thread>>,
    pub last_modified: Option<String>,
    pub poll_interval: Duration,
}

/// One raw `/notifications` page response, before it is parsed.
struct RawPage {
    status: StatusCode,
    last_modified: Option<String>,
    poll_interval: Duration,
    body: String,
}

/// One response with its headers read before the body was consumed, which
/// is the order the allowance and the conditional headers need.
struct Fetched {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

/// The client. A clone shares the allowance cells, so every copy reports
/// what the last response on any of them said.
#[derive(Clone)]
pub struct GitHub {
    octo: Octocrab,
    http: reqwest::Client,
    /// `x-ratelimit-remaining` from the last response, or `UNSEEN`.
    remaining: Arc<AtomicI64>,
    /// `x-ratelimit-reset` from the last response, or `UNSEEN`.
    reset: Arc<AtomicI64>,
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

        Ok(Self {
            octo,
            http,
            remaining: Arc::new(AtomicI64::new(UNSEEN)),
            reset: Arc::new(AtomicI64::new(UNSEEN)),
        })
    }

    /// The rate limit as the last response reported it, on this client or
    /// any clone of it, and `None` until one has.
    pub fn allowance(&self) -> Option<Allowance> {
        let remaining = self.remaining.load(Ordering::Relaxed);
        let reset_epoch = self.reset.load(Ordering::Relaxed);
        if remaining < 0 || reset_epoch < 0 {
            return None;
        }
        Some(Allowance { remaining, reset_epoch })
    }

    /// Keep what a response said about the rate limit. Each header updates
    /// its own cell, so with requests in flight together the pair can come
    /// from two responses; the backfill reads it as a floor, and either
    /// response's numbers serve that.
    fn note_allowance(&self, headers: &HeaderMap) {
        let (remaining, reset) = rate_headers(headers);
        if let Some(remaining) = remaining {
            self.remaining.store(remaining, Ordering::Relaxed);
        }
        if let Some(reset) = reset {
            self.reset.store(reset, Ordering::Relaxed);
        }
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
    async fn fetch_page(&self, page: usize, since: Option<&str>) -> Result<RawPage, Error> {
        let uri =
            format!("/notifications?all=true&participating=false&per_page={PER_PAGE}&page={page}");

        let mut headers = HeaderMap::new();
        if let Some(since) = since.and_then(|s| HeaderValue::from_str(s).ok()) {
            headers.insert(IF_MODIFIED_SINCE, since);
        }

        let fetched = self.get(&uri, Some(headers)).await?;
        let last_modified = header_str(&fetched.headers, LAST_MODIFIED.as_str());
        let poll_interval = header_str(&fetched.headers, "x-poll-interval")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|s| Duration::from_secs(s.max(1)))
            .unwrap_or_else(|| Duration::from_secs(DEFAULT_POLL_SECS));

        Ok(RawPage {
            status: fetched.status,
            last_modified,
            poll_interval,
            body: fetched.body,
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

        let fetched = self.get(&path, None).await?;
        let payload: SubjectPayload =
            serde_json::from_str(&fetched.body).map_err(|e| Error::Decode(e.to_string()))?;

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

    /// Every open pull request in a repository, or `Unchanged` when the tag
    /// from last time still holds. What the list pass makes its rows from.
    pub async fn list_open_pulls(
        &self,
        repo_full_name: &str,
        etag: Option<&str>,
    ) -> Result<Listed<PullRequest>, Error> {
        let path = format!("/repos/{repo_full_name}/pulls?state=open");
        Ok(match self.list_open::<PullPayload>(&path, etag).await? {
            Listed::Unchanged => Listed::Unchanged,
            Listed::Changed { etag, items } => Listed::Changed {
                etag,
                items: items
                    .into_iter()
                    .map(|p| p.into_pull_request(repo_full_name))
                    .collect(),
            },
        })
    }

    /// Every open issue in a repository, the same way. The issues endpoint
    /// lists pull requests too, and those are dropped here rather than
    /// refused as `fetch_issue` does: a list is not a claim about one
    /// number.
    pub async fn list_open_issues(
        &self,
        repo_full_name: &str,
        etag: Option<&str>,
    ) -> Result<Listed<Issue>, Error> {
        let path = format!("/repos/{repo_full_name}/issues?state=open");
        Ok(match self.list_open::<IssuePayload>(&path, etag).await? {
            Listed::Unchanged => Listed::Unchanged,
            Listed::Changed { etag, items } => Listed::Changed {
                etag,
                items: issues_only(items, repo_full_name),
            },
        })
    }

    /// One page of a repository's closed pull requests, oldest updated
    /// first, so an interrupted backfill leaves a contiguous prefix behind.
    pub async fn list_closed_pulls(
        &self,
        repo_full_name: &str,
        page: u32,
    ) -> Result<Page<PullRequest>, Error> {
        let path = format!(
            "/repos/{repo_full_name}/pulls?state=closed&sort=updated&direction=asc\
             &per_page={LIST_PER_PAGE}&page={page}"
        );
        let fetched = self.get(&path, None).await?;
        let payloads: Vec<PullPayload> =
            serde_json::from_str(&fetched.body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(Page {
            newest: payloads.iter().filter_map(|p| p.updated_at).max(),
            items: payloads
                .into_iter()
                .map(|p| p.into_pull_request(repo_full_name))
                .collect(),
            last_page: last_page_of(&fetched.headers),
        })
    }

    /// One page of a repository's closed issues, the same way, and from
    /// `since` once the backfill has been to the end: the issues endpoint
    /// takes one where the pull requests endpoint does not. Pull requests
    /// are dropped as `list_open_issues` drops them.
    pub async fn list_closed_issues(
        &self,
        repo_full_name: &str,
        page: u32,
        since: Option<Timestamp>,
    ) -> Result<Page<Issue>, Error> {
        let mut path = format!(
            "/repos/{repo_full_name}/issues?state=closed&sort=updated&direction=asc\
             &per_page={LIST_PER_PAGE}&page={page}"
        );
        if let Some(since) = since {
            path.push_str(&format!("&since={since}"));
        }
        let fetched = self.get(&path, None).await?;
        let payloads: Vec<IssuePayload> =
            serde_json::from_str(&fetched.body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(Page {
            // Before the pull requests are dropped: they count toward the
            // page and toward `since`, so a cursor that stopped short of
            // them would list them again on every catch-up.
            newest: payloads.iter().map(|p| p.updated_at).max(),
            items: issues_only(payloads, repo_full_name),
            last_page: last_page_of(&fetched.headers),
        })
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

    /// Every page of a list whose first page is asked conditionally.
    ///
    /// Only the first page carries `If-None-Match`: a tag belongs to one
    /// response, and a later page has a tag of its own that nobody stored.
    /// The first page's tag is what comes back when the list fit on it,
    /// since a tag vouches for one page's body and nothing beyond: a list
    /// that ran to a second page hands back no tag, so the next pass asks
    /// unconditionally and sees a closure on the later pages that would
    /// have left the first page's body, and its tag, exactly as they were.
    /// Paging is `list_all`'s: stop at the first short page, cap at a
    /// hundred. The short-page test is on what the server sent, before
    /// the caller drops anything from it.
    async fn list_open<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> Result<Listed<T>, Error> {
        let mut all = Vec::new();
        let mut first_etag = None;
        let mut pages = 0;
        for page in 1..=LIST_MAX_PAGES {
            let headers = if page == 1 { conditional(etag) } else { None };
            let fetched = self
                .get(&format!("{path}&per_page={LIST_PER_PAGE}&page={page}"), headers)
                .await?;
            if page == 1 {
                if fetched.status == StatusCode::NOT_MODIFIED {
                    return Ok(Listed::Unchanged);
                }
                first_etag = header_str(&fetched.headers, ETAG.as_str());
            }
            pages = page;
            let items: Vec<T> = serde_json::from_str(&fetched.body)
                .map_err(|e| Error::Decode(e.to_string()))?;
            let short = items.len() < LIST_PER_PAGE;
            all.extend(items);
            if short {
                break;
            }
        }
        let etag = if pages == 1 { first_etag } else { None };
        Ok(Listed::Changed { etag, items: all })
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
    /// The allowance is noted from the headers first, whatever the status,
    /// because GitHub sends the rate limit headers on a 304 and on a
    /// refusal the same as on an answer.
    async fn get(&self, path: &str, headers: Option<HeaderMap>) -> Result<Fetched, Error> {
        let response = self
            .octo
            ._get_with_headers(path, headers)
            .await
            .map_err(map_octocrab_error)?;

        let status = response.status();
        let headers = response.headers().clone();
        self.note_allowance(&headers);

        if status == StatusCode::NOT_MODIFIED {
            return Ok(Fetched { status, headers, body: String::new() });
        }

        let body = body_text(response).await;
        if !status.is_success() {
            let (remaining, reset) = rate_headers(&headers);
            return Err(classify(status, remaining, reset, body));
        }
        Ok(Fetched { status, headers, body })
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

/// The issues among what the issues endpoint listed, since it lists pull
/// requests too.
fn issues_only(payloads: Vec<IssuePayload>, repo_full_name: &str) -> Vec<Issue> {
    payloads
        .into_iter()
        .filter(|p| !p.is_pull_request())
        .map(|p| p.into_issue(repo_full_name))
        .collect()
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

/// The rate limit headers as a pair: requests remaining, and the reset
/// epoch. Either is `None` when absent or unreadable.
fn rate_headers(headers: &HeaderMap) -> (Option<i64>, Option<i64>) {
    let remaining =
        header_str(headers, "x-ratelimit-remaining").and_then(|v| v.parse::<i64>().ok());
    let reset = header_str(headers, "x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
    (remaining, reset)
}

/// The last page a response's `Link` header names, if it names one.
fn last_page_of(headers: &HeaderMap) -> Option<u32> {
    last_page_from_link(&header_str(headers, LINK.as_str())?)
}

/// The page number of the `rel="last"` entry of a `Link` header.
///
/// GitHub writes `<url>; rel="next", <url>; rel="last"` and leaves `last`
/// out on the last page, so `None` is the answer there and for a list that
/// fits in one page. The number is the URL's own `page` parameter rather
/// than a count of entries, because `per_page` carries a number too.
fn last_page_from_link(link: &str) -> Option<u32> {
    link.split(',').find_map(|entry| {
        let (url, params) = entry.split_once(';')?;
        if !params.split(';').any(|p| p.trim() == r#"rel="last""#) {
            return None;
        }
        let url = url.trim().strip_prefix('<')?.strip_suffix('>')?;
        let (_, query) = url.split_once('?')?;
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            if key != "page" {
                return None;
            }
            value.parse().ok()
        })
    })
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
    // A spent window leaves `remaining` at zero. A secondary limit does not,
    // and says so only in the body, so the body is read for it too.
    let exhausted = rate_remaining == Some(0) || body.to_ascii_lowercase().contains("rate limit");
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

    #[test]
    fn a_403_that_names_the_rate_limit_is_rate_limited_whatever_remains() {
        let body = r#"{"message":"You have exceeded a secondary rate limit."}"#.to_string();
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, Some(42), Some(9), body),
            Error::RateLimited { reset_epoch: Some(9) }
        ));
    }

    #[test]
    fn the_last_page_comes_from_the_link_header_and_only_from_rel_last() {
        let link = r#"<https://api.github.com/repositories/1/pulls?state=closed&per_page=100&page=2>; rel="next", <https://api.github.com/repositories/1/pulls?state=closed&per_page=100&page=34>; rel="last""#;
        assert_eq!(last_page_from_link(link), Some(34));

        // The last page names its neighbours and not itself.
        let tail = r#"<https://api.github.com/repositories/1/pulls?state=closed&per_page=100&page=33>; rel="prev", <https://api.github.com/repositories/1/pulls?state=closed&per_page=100&page=1>; rel="first""#;
        assert_eq!(last_page_from_link(tail), None);
        assert_eq!(last_page_from_link(""), None);
    }

    // Building an Octocrab wants a runtime, hence the tokio test for a
    // check that never touches the network.
    #[tokio::test]
    async fn the_allowance_is_the_last_pair_seen_and_a_clone_shares_it() {
        let gh = GitHub::new("token").unwrap();
        assert_eq!(gh.allowance(), None, "nothing has been seen yet");

        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("4999"));
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("1790000000"));
        gh.note_allowance(&headers);
        assert_eq!(
            gh.allowance(),
            Some(Allowance { remaining: 4999, reset_epoch: 1790000000 })
        );

        let twin = gh.clone();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("4998"));
        twin.note_allowance(&headers);
        assert_eq!(
            gh.allowance().map(|a| a.remaining),
            Some(4998),
            "a clone writes the same cells"
        );

        // A response without the headers leaves the last pair standing.
        gh.note_allowance(&HeaderMap::new());
        assert_eq!(gh.allowance().map(|a| a.remaining), Some(4998));
    }
}
